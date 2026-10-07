use crate::geometry::NativeSizes;
use crate::ipc::{self, Request, SocketLease};
use crate::modes::{ModeKind, WindowMode};
use anyhow::{Context, Result, bail};
use ribbon_core::{Action, Engine, Placement, Rect, Settings, WindowId};
use ribbon_macos::{
    Display, Window,
    backend::{Backend, StickyWindow},
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Options {
    pub selected: Vec<u32>,
    pub all: bool,
    pub exclude_apps: Vec<String>,
    pub dry_run: bool,
    pub settings: Settings,
}
struct GeometryLease {
    modes: BTreeMap<WindowId, WindowMode>,
    originals: BTreeMap<WindowId, Window>,
    logical_originals: BTreeMap<WindowId, Rect>,
    resized: BTreeSet<WindowId>,
    dragged: Option<WindowId>,
}
#[derive(Default)]
struct MouseCapture {
    pressed: bool,
    point: (f64, f64),
    event_window: u32,
    window: Option<WindowId>,
    surface: Option<Rect>,
    current: Option<Rect>,
    shown: Option<Rect>,
    changed: bool,
    corrected: bool,
    resizing: bool,
    left_edge: bool,
    top_edge: bool,
    active: bool,
}
fn pressed_placement<'a>(
    plans: &'a [Placement],
    geometry: &GeometryLease,
    event_window: u32,
    frontmost: i32,
    point: (f64, f64),
) -> Option<&'a Placement> {
    plans.iter().find(|p| {
        geometry.originals.get(&p.window).is_some_and(|w| {
            // At mouse-down the clicked app may not have activated yet. An
            // explicit event target is stronger evidence than frontmost PID.
            if event_window != 0 {
                p.window.0 == event_window
            } else {
                w.pid == frontmost
            }
        }) && p.clip.is_some_and(|r| {
            Rect {
                x: r.x - 8.0,
                y: r.y - 8.0,
                width: r.width + 16.0,
                height: r.height + 16.0,
            }
            .contains(point.0, point.1)
        })
    })
}
fn chrome_hit(frame: Rect, point: (f64, f64)) -> bool {
    let (x, y) = point;
    y <= frame.y + 36.0
        || x <= frame.x + 8.0
        || x >= frame.x + frame.width - 8.0
        || y >= frame.y + frame.height - 8.0
}
impl MouseCapture {
    fn update(
        &mut self,
        engine: &Engine,
        geometry: &GeometryLease,
        committed: &[Placement],
    ) -> Option<WindowId> {
        let sample = ribbon_macos::mouse_state();
        if !ribbon_macos::left_mouse_down() {
            *self = Self::default();
            return None;
        }
        if !self.pressed {
            self.pressed = true;
            self.point = if sample.down != 0 {
                (sample.x, sample.y)
            } else {
                ribbon_macos::pointer()
            };
            self.event_window = if sample.down != 0 { sample.window } else { 0 };
        }
        // Activation can arrive after mouse-down. Retry with the original
        // press position, never retarget an established capture under a drag.
        if self.window.is_none() {
            let plans = engine.placements();
            if let Some(p) = pressed_placement(
                &plans,
                geometry,
                self.event_window,
                ribbon_macos::frontmost_pid(),
                self.point,
            ) {
                self.window = Some(p.window);
                if let Some(w) = geometry.originals.get(&p.window)
                    && let Ok((surface, shown)) = ribbon_macos::window_drag_geometry(w.id, w.pid)
                {
                    self.surface = Some(surface);
                    self.current = Some(surface);
                    // The owner may already have moved before this frame reads
                    // the press. Anchor the grab to our last committed display,
                    // not a transform that may contain the owner's first jump.
                    self.shown = Some(
                        committed
                            .iter()
                            .find(|old| {
                                old.window == p.window && old.native_space == p.native_space
                            })
                            .map_or(shown, |old| Rect {
                                x: old.frame.x,
                                y: old.frame.y,
                                width: surface.width,
                                height: surface.height,
                            }),
                    );
                    if let Some(anchor) = self.shown {
                        self.corrected = (surface.x - anchor.x).abs() <= 2.0
                            && (surface.y - anchor.y).abs() <= 2.0
                            && (shown.x - anchor.x).abs() <= 2.0
                            && (shown.y - anchor.y).abs() <= 2.0;
                    }
                }
                let frame = self.shown.unwrap_or(p.frame);
                self.left_edge = self.point.0 <= frame.x + 8.0;
                self.top_edge = self.point.1 <= frame.y + 8.0;
                self.resizing = self.left_edge
                    || self.top_edge
                    || self.point.0 >= frame.x + frame.width - 8.0
                    || self.point.1 >= frame.y + frame.height - 8.0;
                self.active = chrome_hit(frame, self.point);
            }
        }
        if sample.dragged != 0
            && let Some(id) = self.window
            && let Some(w) = geometry.originals.get(&id)
            && let Some(before) = self.surface
            && let Ok((now, shown)) = ribbon_macos::window_drag_geometry(id, w.pid)
        {
            let changed = (now.x - before.x).abs() > 2.0
                || (now.y - before.y).abs() > 2.0
                || (now.width - before.width).abs() > 2.0
                || (now.height - before.height).abs() > 2.0;
            let moved_transform = self.shown.is_some_and(|start| {
                (shown.x - start.x).abs() > 2.0 || (shown.y - start.y).abs() > 2.0
            });
            self.changed |= changed || moved_transform;
            self.active |= changed || moved_transform;
            self.current = Some(now);
        }
        self.window.filter(|_| self.active)
    }
    fn frame(&self, viewport: Rect) -> Option<Rect> {
        self.frame_for_pointer(ribbon_macos::pointer(), viewport)
    }
    fn frame_for_pointer(&self, pointer: (f64, f64), viewport: Rect) -> Option<Rect> {
        if !self.changed || self.corrected {
            return None;
        }
        let current = self.current?;
        let mut frame = drag_frame(
            self.shown?,
            self.point,
            pointer,
            current,
            self.resizing,
            self.left_edge,
            self.top_edge,
        );
        // Prefer an accepted owner position to a pointer sample that runs
        // slightly ahead of AppKit's first drag transaction.
        if !self.resizing
            && (current.x - frame.x).abs() <= 4.0
            && (current.y - frame.y).abs() <= 4.0
        {
            frame.x = current.x;
            frame.y = current.y;
        }
        frame.y = frame.y.max(viewport.y);
        Some(frame)
    }
}
fn drag_frame(
    start: Rect,
    press: (f64, f64),
    pointer: (f64, f64),
    current: Rect,
    resizing: bool,
    left: bool,
    top: bool,
) -> Rect {
    Rect {
        x: start.x
            + if resizing {
                if left {
                    start.width - current.width
                } else {
                    0.0
                }
            } else {
                pointer.0 - press.0
            },
        y: start.y
            + if resizing {
                if top {
                    start.height - current.height
                } else {
                    0.0
                }
            } else {
                pointer.1 - press.1
            },
        width: current.width,
        height: current.height,
    }
}
#[derive(Default)]
struct InventorySnapshot {
    windows: Vec<Window>,
    ready: BTreeMap<WindowId, (i32, Rect)>,
    members: BTreeMap<i32, BTreeSet<WindowId>>,
}
impl InventorySnapshot {
    fn collect(options: &Options, known: &BTreeMap<WindowId, i32>) -> Result<Self> {
        let windows = ribbon_macos::windows()?;
        Ok(Self::probe(
            windows,
            options,
            known,
            ribbon_macos::probe_application,
        ))
    }
    fn probe(
        windows: Vec<Window>,
        options: &Options,
        known: &BTreeMap<WindowId, i32>,
        mut probe: impl FnMut(i32, &[u32]) -> Result<ribbon_macos::ApplicationProbe>,
    ) -> Self {
        let mut targets: BTreeMap<i32, Vec<u32>> =
            known.values().map(|pid| (*pid, Vec::new())).collect();
        for w in &windows {
            if !known.contains_key(&w.id)
                && normal_window_candidate(w)
                && !app_excluded(w, &options.exclude_apps)
                && (options.all || options.selected.contains(&w.id.0))
                && w.surface_bounds.is_none_or(|b| {
                    (b.width - w.bounds.width).abs() <= 2.0
                        && (b.height - w.bounds.height).abs() <= 2.0
                })
            {
                targets.entry(w.pid).or_default().push(w.id.0);
            }
        }
        let mut snapshot = Self {
            windows,
            ..Self::default()
        };
        if !options.dry_run {
            for (pid, candidates) in targets {
                // A failed AX query is unknown membership, never an empty app.
                if let Ok(probe) = probe(pid, &candidates) {
                    snapshot
                        .members
                        .insert(pid, probe.members.into_iter().collect());
                    for ready in probe.ready {
                        snapshot.ready.insert(ready.id, (pid, ready.geometry));
                    }
                }
            }
        }
        snapshot
    }
}
/// Blocking AX discovery and retained-window membership queries live here,
/// away from rendering, input, and the main-thread observer callbacks.
struct WindowInventory {
    request: Option<mpsc::SyncSender<u64>>,
    result: mpsc::Receiver<(u64, Result<InventorySnapshot>)>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: bool,
    generation: u64,
    dirty: bool,
    known: Arc<Mutex<BTreeMap<WindowId, i32>>>,
}
impl WindowInventory {
    fn start(options: Options) -> Self {
        let known = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_known = Arc::clone(&known);
        let (request, requests) = mpsc::sync_channel(1);
        let (results, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            while let Ok(generation) = requests.recv() {
                let known = worker_known
                    .lock()
                    .expect("inventory tracking poisoned")
                    .clone();
                if results
                    .send((generation, InventorySnapshot::collect(&options, &known)))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            request: Some(request),
            result,
            worker: Some(worker),
            pending: false,
            generation: 0,
            dirty: false,
            known,
        }
    }
    fn track(&self, geometry: &GeometryLease) {
        *self.known.lock().expect("inventory tracking poisoned") = geometry
            .originals
            .iter()
            .map(|(id, w)| (*id, w.pid))
            .collect();
    }
    fn request(&mut self) -> Result<()> {
        if !self.pending {
            self.request
                .as_ref()
                .context("Inventory stopped")?
                .send(self.generation)?;
            self.pending = true;
            self.dirty = false;
        } else {
            self.dirty = true;
        }
        Ok(())
    }
    fn poll(&mut self) -> Result<Option<InventorySnapshot>> {
        let snapshot = match self.result.try_recv() {
            Ok((generation, result)) => {
                self.pending = false;
                if generation == self.generation {
                    match result {
                        Ok(windows) => Some(windows),
                        Err(error) => {
                            // A failed metadata read is not an empty desktop.
                            // Keep layouts and leases, retry on the next poll.
                            eprintln!("Window inventory deferred: {error:#}");
                            self.dirty = false;
                            None
                        }
                    }
                } else {
                    self.dirty = true;
                    None
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => bail!("Inventory worker stopped"),
        };
        if self.dirty && !self.pending {
            self.request()?;
        }
        Ok(snapshot)
    }
    fn invalidate(&mut self) {
        // A snapshot taken before an AX write must not undo its accepted size.
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }
}
impl Drop for WindowInventory {
    fn drop(&mut self) {
        self.request.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for GeometryLease {
    fn drop(&mut self) {
        for id in &self.resized {
            if let Some(w) = self.originals.get(id)
                && ribbon_macos::window_owner(*id) == w.pid
                && let Some(logical) = self.logical_originals.get(id)
                && let Err(e) = ribbon_macos::restore_window(w, *logical)
            {
                eprintln!("Geometry restore: {e:#}");
            }
        }
    }
}
fn observe_native_focus(
    engine: &mut Engine,
    geometry: &GeometryLease,
    options: &Options,
    reveal: bool,
) {
    if options.dry_run {
        return;
    }
    let pid = ribbon_macos::frontmost_pid();
    // Exclusions are checked before any AX query, including focus observation.
    if geometry
        .originals
        .values()
        .any(|w| w.pid == pid && !app_excluded(w, &options.exclude_apps))
        && let Some(id) = ribbon_macos::focused_window(pid)
    {
        // App activation and Space switching select windows automatically.
        // Observe that selection without overriding the saved viewport. Only a
        // recent click in the selected window requests native focus reveal;
        // explicit WM focus commands already reveal through Engine::apply.
        let click_age = ribbon_macos::left_mouse_down_age();
        let pointer = ribbon_macos::pointer();
        let clicked = click_age.is_finite()
            && (0.0..=0.25).contains(&click_age)
            && engine.placements().iter().any(|p| {
                p.window == id && p.clip.is_some_and(|c| c.contains(pointer.0, pointer.1))
            });
        let _ = engine.observe_focus(id, reveal && clicked);
    }
}
fn forget_closed(engine: &mut Engine, geometry: &mut GeometryLease, id: WindowId) -> Result<()> {
    if engine.window_ids().contains(&id) {
        engine.remove_window(id)?;
    }
    geometry.originals.remove(&id);
    geometry.logical_originals.remove(&id);
    geometry.modes.remove(&id);
    geometry.resized.remove(&id);
    ribbon_macos::forget_window(id);
    Ok(())
}
fn display_for<'a>(window: &Window, displays: &'a [Display]) -> Option<&'a Display> {
    let bounds = window_bounds(window);
    displays
        .iter()
        .filter(|d| {
            !d.native_fullscreen && (window.sticky || window.native_spaces == vec![d.native_space])
        })
        .max_by(|a, b| {
            let area = |d: &Display| {
                bounds
                    .intersection(d.frame)
                    .map_or(0.0, |r| r.width * r.height)
            };
            area(a).total_cmp(&area(b))
        })
        .filter(|d| bounds.intersection(d.frame).is_some())
}
fn window_bounds(window: &Window) -> Rect {
    // Mission Control and compositor clipping change presentation metadata.
    // Width, monitor selection and saved geometry belong to the real surface.
    window
        .surface_bounds
        .filter(|r| r.valid())
        .unwrap_or(window.bounds)
}
fn app_excluded(window: &Window, patterns: &[String]) -> bool {
    identity_excluded(&window.app, &window.bundle_id, patterns)
}
fn identity_excluded(app: &str, bundle_id: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix('*') {
            bundle_id.starts_with(prefix) || app.starts_with(prefix)
        } else {
            bundle_id == *pattern || app == *pattern
        }
    })
}
fn refresh_observers(
    source: &ribbon_macos::EventSource,
    watching: &mut BTreeSet<i32>,
    options: &Options,
) -> Result<()> {
    let eligible: BTreeSet<i32> = if options.all {
        ribbon_macos::applications()?
            .into_iter()
            .filter(|a| !identity_excluded(&a.app, &a.bundle_id, &options.exclude_apps))
            .map(|a| a.pid)
            .collect()
    } else {
        ribbon_macos::windows()?
            .into_iter()
            .filter(|w| {
                options.selected.contains(&w.id.0) && !app_excluded(w, &options.exclude_apps)
            })
            .map(|w| w.pid)
            .collect()
    };
    for pid in watching.difference(&eligible) {
        source.unwatch(*pid);
    }
    watching.retain(|pid| eligible.contains(pid));
    for pid in eligible.difference(watching).copied().collect::<Vec<_>>() {
        if source.watch(pid) {
            watching.insert(pid);
        }
    }
    Ok(())
}
fn normal_window_candidate(window: &Window) -> bool {
    let bounds = window_bounds(window);
    // Display utilities may expose 1x1 normal-layer helper surfaces. They
    // are not user app windows and must never be expanded into a column.
    window.layer == 0
        && bounds.valid()
        && bounds.width >= 100.0
        && bounds.height >= 100.0
        && window.onscreen
        && window.pid != std::process::id() as i32
}
fn resize_anchor(viewport: Rect, original: Rect, width: f64, height: f64) -> Rect {
    // AppKit may reserve a menu-bar strip even when NSScreen reports the full
    // secondary display as usable. Keep a valid original position when it fits
    // the new size; the Dock transform independently owns visual placement.
    Rect {
        x: original.x.clamp(
            viewport.x,
            (viewport.x + viewport.width - width).max(viewport.x),
        ),
        y: original.y.clamp(
            viewport.y,
            (viewport.y + viewport.height - height).max(viewport.y),
        ),
        width,
        height,
    }
}
fn release_candidates(
    plans: Vec<Placement>,
    committed: &[Placement],
    resized: &BTreeSet<WindowId>,
) -> Vec<Placement> {
    plans
        .into_iter()
        .filter(|p| {
            resized.contains(&p.window) || committed.iter().any(|old| old.window == p.window)
        })
        .collect()
}
fn released_frame(engine: &Engine, monitor: &str, mut frame: Rect, index: usize) -> Rect {
    let viewport = engine.monitors[monitor].viewport;
    let inner = Rect {
        x: viewport.x + engine.settings.left_margin(),
        y: viewport.y + engine.settings.top_margin(),
        width: (viewport.width - engine.settings.left_margin() - engine.settings.right_margin())
            .max(100.0),
        height: (viewport.height - engine.settings.top_margin() - engine.settings.bottom_margin())
            .max(100.0),
    };
    if frame.intersection(inner).is_none() {
        frame.x = inner.x + (index % 8) as f64 * 24.0;
        frame.y = inner.y + (index % 8) as f64 * 8.0;
    }
    resize_anchor(
        inner,
        frame,
        frame.width.min(inner.width),
        frame.height.min(inner.height),
    )
}
fn synchronize(
    engine: &mut Engine,
    geometry: &mut GeometryLease,
    options: &Options,
    displays: &[Display],
    sizes: &mut NativeSizes,
    snapshot: (InventorySnapshot, bool),
) -> Result<()> {
    let (snapshot, reveal_focus) = snapshot;
    let InventorySnapshot {
        windows: mut inventory,
        ready,
        members,
    } = snapshot;
    for window in &mut inventory {
        window.bounds = window_bounds(window);
    }
    // Only a cached destination has a saved offset; initial discovery should
    // reveal the native active window normally.
    let context_changed = displays.iter().any(|d| {
        engine.monitors.get(&d.id).is_some_and(|m| {
            (m.native_space != d.native_space && m.contexts.contains_key(&d.native_space))
                || m.suspended != d.native_fullscreen
        })
    });
    update_contexts(engine, displays)?;
    for id in geometry.originals.keys().copied().collect::<Vec<_>>() {
        let old = geometry.originals.get(&id);
        if old.is_some_and(|w| members.get(&w.pid).is_some_and(|ids| !ids.contains(&id)))
            && !inactive_window(engine, geometry, id)
        {
            forget_closed(engine, geometry, id)?;
            sizes.remove(&id);
            continue;
        }
        if !inventory
            .iter()
            .any(|w| w.id == id && old.is_none_or(|o| o.pid == w.pid))
            && (options.dry_run || old.is_none_or(|w| ribbon_macos::window_owner(id) != w.pid))
        {
            forget_closed(engine, geometry, id)?;
        }
    }
    if geometry.dragged.is_none() {
        observe_native_focus(engine, geometry, options, reveal_focus && !context_changed);
    }
    // CG inventories are in stacking order; window IDs give deterministic
    // creation order when several windows arrive between discovery polls.
    inventory.sort_by_key(|w| w.id);
    for w in inventory {
        if app_excluded(&w, &options.exclude_apps) {
            continue;
        }
        if geometry.modes.get(&w.id).is_some_and(|m| !m.wants_tile()) {
            if let Some(original) = geometry.originals.get_mut(&w.id)
                && original.pid == w.pid
            {
                original.native_spaces = w.native_spaces;
            }
            continue;
        }
        if let Some((monitor, space)) = engine.window_context(w.id) {
            if let Some(display) = display_for(&w, displays)
                && geometry.dragged != Some(w.id)
                && (monitor != display.id || space != display.native_space)
            {
                engine.relocate_window(&display.id, w.id)?;
            }
            if !options.dry_run
                && w.onscreen
                && let Some(surface) = w.surface_bounds.filter(|r| r.valid())
                && let Some(&(pid, width, height)) = sizes.get(&w.id)
                && pid == w.pid
            {
                // Compare with the last accepted AX size, not startup geometry.
                // A pending WM command takes precedence over a stale native sample.
                let plan = engine.placements().into_iter().find(|p| {
                    p.window == w.id
                        && engine.monitors[&p.monitor].native_space == p.native_space
                        && !engine.monitors[&p.monitor].suspended
                });
                if let Some(plan) = plan {
                    let mut accepted = (pid, width, height);
                    if (plan.frame.width - width).abs() <= 2.0
                        && (surface.width - width).abs() > 2.0
                        && (100.0..=10000.0).contains(&surface.width)
                    {
                        engine.observe_width(w.id, surface.width)?;
                        accepted.1 = surface.width;
                    }
                    if (plan.frame.height - height).abs() <= 2.0
                        && (surface.height - height).abs() > 2.0
                        && surface.height >= 100.0
                    {
                        let _ = engine.resize_row(&plan.monitor, w.id, surface.height);
                        accepted.2 = surface.height;
                    }
                    sizes.insert(w.id, accepted);
                }
            }
            continue;
        }
        if geometry.dragged.is_some() || !normal_window_candidate(&w) {
            continue;
        }
        if !options.all && !options.selected.contains(&w.id.0) {
            continue;
        }
        let Some(display) = display_for(&w, displays) else {
            continue;
        };
        if !options.dry_run {
            let Some(&(pid, logical)) = ready.get(&w.id) else {
                continue;
            };
            if pid != w.pid || ribbon_macos::window_owner(w.id) != pid {
                continue;
            }
            geometry.logical_originals.insert(w.id, logical);
        }
        let width = engine.settings.preserve_window_width.then(|| {
            w.bounds
                .width
                .min(
                    (display.viewport.width
                        - engine.settings.left_margin()
                        - engine.settings.right_margin())
                    .max(100.0),
                )
                .clamp(100.0, 10000.0)
        });
        let mode = *geometry.modes.entry(w.id).or_insert(WindowMode {
            sticky: w.sticky,
            ..WindowMode::default()
        });
        if mode.wants_tile() {
            engine.add_window(&display.id, w.id, width)?;
        }
        geometry.originals.insert(w.id, w);
    }
    observe_native_focus(engine, geometry, options, reveal_focus && !context_changed);
    Ok(())
}

fn update_contexts(engine: &mut Engine, displays: &[Display]) -> Result<()> {
    for d in displays {
        engine.update_monitor(&d.id, d.viewport, d.native_space, d.native_fullscreen)?;
    }
    for m in engine.monitors.values_mut() {
        if !displays.iter().any(|d| d.id == m.id) {
            m.suspended = true;
        }
    }
    Ok(())
}

fn inactive_window(engine: &Engine, geometry: &GeometryLease, id: WindowId) -> bool {
    if let Some((mid, sid)) = engine.window_context(id) {
        let m = &engine.monitors[mid];
        return m.native_space != sid || m.suspended;
    }
    geometry.originals.get(&id).is_some_and(|w| {
        !geometry.modes.get(&id).is_some_and(|mode| mode.sticky)
            && !w.native_spaces.is_empty()
            && !engine
                .monitors
                .values()
                .any(|m| !m.suspended && w.native_spaces.contains(&m.native_space))
    })
}

fn monitor_for(engine: &Engine, requested: Option<String>) -> Result<String> {
    if let Some(id) = requested {
        if !engine.monitors.contains_key(&id) {
            bail!("Unknown monitor UUID");
        }
        return Ok(id);
    }
    let (x, y) = ribbon_macos::pointer();
    engine
        .monitors
        .values()
        .find(|m| m.viewport.contains(x, y) && !m.suspended)
        .or_else(|| engine.monitors.values().find(|m| !m.suspended))
        .map(|m| m.id.clone())
        .context("No active monitor")
}
fn monitor_viewports(engine: &Engine) -> BTreeMap<String, Rect> {
    engine
        .monitors
        .iter()
        .map(|(id, m)| (id.clone(), m.viewport))
        .collect()
}
fn sticky_leases(geometry: &GeometryLease) -> Vec<StickyWindow> {
    geometry
        .modes
        .iter()
        .filter(|(_, m)| m.sticky_leased)
        .filter_map(|(id, m)| {
            Some(StickyWindow {
                wid: id.0,
                pid: geometry.originals.get(id)?.pid,
                enabled: m.sticky,
            })
        })
        .collect()
}
fn set_mode(
    engine: &mut Engine,
    geometry: &mut GeometryLease,
    backend: Option<&Backend>,
    sizes: &mut NativeSizes,
    change: (Option<WindowId>, ModeKind, Option<bool>),
    dry_run: bool,
) -> Result<()> {
    let (window, kind, enabled) = change;
    let id = window
        .or_else(|| {
            let pid = ribbon_macos::frontmost_pid();
            geometry
                .originals
                .values()
                .any(|w| w.pid == pid)
                .then(|| ribbon_macos::focused_window(pid))
                .flatten()
        })
        .context("No focused tracked window; pass --window ID")?;
    let original = geometry
        .originals
        .get(&id)
        .context("Unknown tracked window")?
        .clone();
    let old = *geometry.modes.get(&id).context("Missing window mode")?;
    let mut next = old.change(kind, enabled);
    let current = ribbon_macos::windows()?
        .into_iter()
        .find(|w| w.id == id && w.pid == original.pid)
        .context("Window owner changed")?;
    if !current.onscreen {
        bail!("Mode changes require a visible window");
    }
    if matches!(kind, ModeKind::Sticky) && next.sticky != old.sticky && !dry_run {
        backend
            .context("Dock backend required")?
            .set_sticky(&StickyWindow {
                wid: id.0,
                pid: original.pid,
                enabled: next.sticky,
            })?;
        next.sticky_leased = true;
    }
    let before = engine.clone();
    let old_logical = geometry.logical_originals.get(&id).copied();
    geometry.modes.insert(id, next);
    let transition = (|| -> Result<()> {
        if !next.wants_tile() && engine.window_ids().contains(&id) {
            if !dry_run {
                let backend = backend.context("Dock backend required")?;
                let held = engine.placements();
                let mut released = held
                    .iter()
                    .find(|p| p.window == id)
                    .context("Missing placement")?
                    .clone();
                let shown = ribbon_macos::window_presentation(id, original.pid)?;
                released.frame = released_frame(engine, &released.monitor, shown, 0);
                released.clip = Some(released.frame);
                let owners = geometry
                    .originals
                    .iter()
                    .map(|(id, w)| (*id, w.pid))
                    .collect();
                ribbon_macos::resize_window_observed(id, original.pid, released.frame, || {
                    backend.frame_with_sticky_in_viewports(
                        &held,
                        &owners,
                        &sticky_leases(geometry),
                        &monitor_viewports(engine),
                    )
                })?;
                backend.detach(&released, original.pid)?;
            }
            engine.remove_window(id)?;
        } else if next.wants_tile() && !engine.window_ids().contains(&id) {
            let mut current = ribbon_macos::windows()?
                .into_iter()
                .find(|w| w.id == id && w.pid == original.pid)
                .context("Window disappeared")?;
            current.bounds = window_bounds(&current);
            if let Some(display) = display_for(&current, &ribbon_macos::displays()?) {
                let width = current.bounds.width.min(
                    (display.viewport.width
                        - engine.settings.left_margin()
                        - engine.settings.right_margin())
                    .max(100.0),
                );
                engine.add_window(&display.id, id, Some(width))?;
                if !dry_run {
                    geometry
                        .logical_originals
                        .insert(id, ribbon_macos::window_geometry(id, current.pid)?);
                }
                geometry.originals.insert(id, current);
            }
        }
        if let Some(backend) = backend {
            let owners = geometry
                .originals
                .iter()
                .map(|(id, w)| (*id, w.pid))
                .collect();
            backend.frame_with_sticky_in_viewports(
                &engine.placements(),
                &owners,
                &sticky_leases(geometry),
                &monitor_viewports(engine),
            )?;
        }
        Ok(())
    })();
    sizes.remove(&id);
    if let Err(error) = transition {
        *engine = before;
        geometry.modes.insert(id, old);
        geometry.originals.insert(id, original.clone());
        if let Some(logical) = old_logical {
            geometry.logical_originals.insert(id, logical);
        } else {
            geometry.logical_originals.remove(&id);
        }
        if next.sticky != old.sticky
            && !dry_run
            && let Some(backend) = backend
        {
            let _ = backend.set_sticky(&StickyWindow {
                wid: id.0,
                pid: original.pid,
                enabled: old.sticky,
            });
        }
        return Err(error);
    }
    if !next.wants_tile() {
        geometry.resized.remove(&id);
    }
    Ok(())
}
fn handle(
    engine: &mut Engine,
    request: Request,
    dry_run: bool,
    geometry: &mut GeometryLease,
    backend: Option<&Backend>,
    sizes: &mut NativeSizes,
    quit: &mut bool,
) -> Result<serde_json::Value> {
    match request {
        Request::Status {} => {
            let pid = ribbon_macos::frontmost_pid();
            let native_focused_window = geometry.dragged.or_else(|| {
                (!dry_run && geometry.originals.values().any(|w| w.pid == pid))
                    .then(|| ribbon_macos::focused_window(pid))
                    .flatten()
            });
            let original_geometry: BTreeMap<_, _> = geometry
                .originals
                .iter()
                .map(|(id, w)| (*id, json!({"pid":w.pid,"bounds":w.bounds})))
                .collect();
            return Ok(
                json!({"ok":true,"mode":if dry_run{"dry_run"}else{"live"},"state":engine,"original_geometry":original_geometry,"window_modes":geometry.modes,"native_focused_window":native_focused_window,"mouse_hold":geometry.dragged,"native_size_requests":sizes.requests,"native_overview":!dry_run&&ribbon_macos::mission_control_active(),"placements":engine.placements()}),
            );
        }
        Request::Quit {} => *quit = true,
        Request::SetMode {
            window,
            kind,
            enabled,
        } => {
            set_mode(
                engine,
                geometry,
                backend,
                sizes,
                (window, kind, enabled),
                dry_run,
            )?;
        }
        Request::FocusMonitor { target } => {
            let current = engine
                .focused_monitor()
                .map(str::to_owned)
                .map(Ok)
                .unwrap_or_else(|| monitor_for(engine, None))?;
            let mut monitors: Vec<_> = engine.monitors.values().filter(|m| !m.suspended).collect();
            let displays = ribbon_macos::displays()?;
            monitors.sort_by_key(|m| {
                displays
                    .iter()
                    .position(|d| d.id == m.id)
                    .unwrap_or(usize::MAX)
            });
            let i = monitors
                .iter()
                .position(|m| m.id == current)
                .context("Current monitor missing")?;
            let center = |r: Rect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
            let selected = match target.as_str() {
                "first" => monitors.first().copied(),
                "last" => monitors.last().copied(),
                "prev" | "previous" => i.checked_sub(1).and_then(|i| monitors.get(i)).copied(),
                "next" => monitors.get(i + 1).copied(),
                "mouse" => {
                    let (x, y) = ribbon_macos::pointer();
                    monitors.iter().copied().find(|m| m.viewport.contains(x, y))
                }
                "recent" => engine
                    .recent_monitor(&current)
                    .and_then(|mid| engine.monitors.get(mid)),
                "left" | "west" | "right" | "east" | "up" | "north" | "down" | "south" => {
                    let (x, y) = center(monitors[i].viewport);
                    monitors
                        .iter()
                        .copied()
                        .filter(|m| {
                            let (dx, dy) = center(m.viewport);
                            match target.as_str() {
                                "left" | "west" => dx < x,
                                "right" | "east" => dx > x,
                                "up" | "north" => dy < y,
                                _ => dy > y,
                            }
                        })
                        .min_by(|a, b| {
                            let distance = |r: Rect| {
                                let (dx, dy) = center(r);
                                (dx - x).powi(2) + (dy - y).powi(2)
                            };
                            distance(a.viewport).total_cmp(&distance(b.viewport))
                        })
                }
                _ => monitors
                    .iter()
                    .copied()
                    .find(|m| m.id == target)
                    .or_else(|| {
                        target
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| i.checked_sub(1))
                            .and_then(|i| monitors.get(i))
                            .copied()
                    }),
            }
            .context("No monitor matches this selector")?;
            let id = selected
                .layout()
                .focused_window()
                .context("Destination monitor has no managed window")?;
            engine.focus_window(id)?;
            if !dry_run {
                let w = geometry
                    .originals
                    .get(&id)
                    .context("Window metadata missing")?;
                ribbon_macos::focus_window(id, w.pid)?;
            }
        }
        Request::Configure { name, value } => {
            let mut settings = engine.settings.clone();
            if let Some(v) = value {
                match name.as_str() {
                    "gap" => settings.gap = v,
                    "padding_left" => settings.padding_left = Some(v),
                    "padding_right" => settings.padding_right = Some(v),
                    "padding_top" => settings.padding_top = Some(v),
                    "padding_bottom" => settings.padding_bottom = Some(v),
                    "column_width" => settings.column_width = v,
                    "animation_duration" => settings.animation_duration = v,
                    _ => bail!("Unknown setting: {name}"),
                }
                engine.update_settings(settings)?;
            }
            let value = match name.as_str() {
                "gap" => engine.settings.gap,
                "padding_left" => engine.settings.left_margin(),
                "padding_right" => engine.settings.right_margin(),
                "padding_top" => engine.settings.top_margin(),
                "padding_bottom" => engine.settings.bottom_margin(),
                "column_width" => engine.settings.column_width,
                "animation_duration" => engine.settings.animation_duration,
                _ => bail!("Unknown setting: {name}"),
            };
            return Ok(json!({"ok":true,"value":value}));
        }
        Request::Apply { monitor, action } => {
            let monitor = monitor_for(engine, monitor)?;
            let before = engine.focused_window(&monitor);
            let explicit_focus = matches!(action, Action::Focus { .. });
            engine.apply(&monitor, &action)?;
            if !dry_run
                && let Some(id) = engine.focused_window(&monitor)
                && (explicit_focus || Some(id) != before)
            {
                let w = geometry
                    .originals
                    .get(&id)
                    .context("Focused window metadata missing")?;
                ribbon_macos::focus_window(id, w.pid)?;
            }
        }
        Request::FocusWindow { window } => {
            if engine.window_ids().contains(&window) {
                engine.focus_window(window)?;
            } else if !geometry.originals.contains_key(&window) {
                bail!("Unknown window");
            } else if !dry_run
                && !ribbon_macos::windows()?
                    .iter()
                    .any(|w| w.id == window && w.onscreen)
            {
                bail!("Floating window is outside the visible desktop");
            }
            if !dry_run {
                let w = geometry
                    .originals
                    .get(&window)
                    .context("Window metadata missing")?;
                ribbon_macos::focus_window(window, w.pid)?;
            }
        }
    }
    Ok(json!({"ok":true}))
}
pub fn run(options: Options) -> Result<()> {
    run_impl(options, false)
}
/// Service gate has already verified live AX access from this same process.
pub fn run_ready(options: Options) -> Result<()> {
    run_impl(options, true)
}
fn run_impl(options: Options, permission_confirmed: bool) -> Result<()> {
    options.settings.validate()?;
    if !options.all && options.selected.is_empty() {
        bail!("Select explicit --windows IDs or opt in with --all");
    }
    if !options.dry_run && !permission_confirmed && !ribbon_macos::accessibility_trusted() {
        bail!("Accessibility permission is required for live mode; run doctor first");
    }
    let socket = SocketLease::bind()?;
    // Declare backend before geometry so geometry is restored before backend releases transforms.
    let backend = if options.dry_run {
        None
    } else {
        let backend = Backend::connect()?;
        backend.require_live_version()?;
        // Fail before recording or changing any AX geometry if another
        // controller still owns the Dock lease.
        backend.heartbeat()?;
        Some(backend)
    };
    let mut geometry = GeometryLease {
        modes: BTreeMap::new(),
        originals: BTreeMap::new(),
        logical_originals: BTreeMap::new(),
        resized: BTreeSet::new(),
        dragged: None,
    };
    let mut engine = Engine::new(options.settings.clone())?;
    let mut displays = ribbon_macos::displays()?;
    if displays.is_empty() || displays.iter().any(|d| d.native_space.0 == 0) {
        bail!("Could not resolve current native Space context");
    }
    let mut sizes = NativeSizes::default();
    let events = (!options.dry_run).then(ribbon_macos::EventSource::default);
    let mut watching = BTreeSet::new();
    if let Some(source) = &events {
        refresh_observers(source, &mut watching, &options)?;
    }
    let initial_overview = !options.dry_run && ribbon_macos::mission_control_active();
    update_contexts(&mut engine, &displays)?;
    if options.dry_run && !initial_overview {
        synchronize(
            &mut engine,
            &mut geometry,
            &options,
            &displays,
            &mut sizes,
            (
                InventorySnapshot {
                    windows: ribbon_macos::windows()?,
                    ..InventorySnapshot::default()
                },
                true,
            ),
        )?;
    }
    for requested in &options.selected {
        if !initial_overview && ribbon_macos::window_owner(WindowId(*requested)) == 0 {
            bail!(
                "Window {requested} is unavailable, minimized, sticky, or outside the current desktop context"
            );
        }
    }
    let running = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&running);
    ctrlc::set_handler(move || signal.store(false, Ordering::Relaxed))?;
    let frame_duration = Duration::from_secs_f64(1.0 / options.settings.frame_rate as f64);
    let mut last_tick = Instant::now();
    let mut last_inventory = last_tick;
    let mut last_send = last_tick - Duration::from_secs(1);
    let mut last_focus = last_tick;
    let mut last_watch_refresh = last_tick;
    let event_trace = std::env::var_os("RIBBONWM_EVENT_TRACE").is_some();
    let frame_trace = std::env::var_os("RIBBONWM_FRAME_TRACE").is_some();
    let mut last_frame = Vec::new();
    let mut committed: Vec<Placement> = Vec::new();
    let mut quit = false;
    let mut input: Option<ribbon_macos::input::InputSource> = None;
    let mut last_input_attempt = last_tick - Duration::from_secs(5);
    let mut gesture_frontmost = (-1, true);
    let mut inventory = WindowInventory::start(options.clone());
    inventory.request()?;
    let mut mouse = MouseCapture::default();
    let mut overview = false;
    let mut overview_resume = Instant::now();
    eprintln!(
        "RibbonWM {}: {} windows; socket {}",
        if options.dry_run { "dry run" } else { "live" },
        engine.window_ids().len(),
        ipc::socket_path().display()
    );
    let frame_result = (|| -> Result<()> {
        'frames: while running.load(Ordering::Relaxed) && !quit {
            let start = Instant::now();
            let notifications = events.as_ref().map_or(0, ribbon_macos::EventSource::drain);
            inventory.track(&geometry);
            let in_overview = !options.dry_run && ribbon_macos::mission_control_active();
            if in_overview && !overview {
                if let Some(backend) = &backend {
                    backend.overview()?;
                }
                if let Some(input) = &mut input {
                    input.cancel();
                }
                inventory.invalidate();
                overview = true;
            } else if !in_overview && overview {
                overview = false;
                overview_resume = start + Duration::from_millis(200);
                inventory.invalidate();
                last_frame.clear();
                mouse = MouseCapture::default();
            }
            let suspended = overview || start < overview_resume;
            let previous_drag = geometry.dragged;
            geometry.dragged = if options.dry_run || suspended {
                None
            } else {
                mouse.update(&engine, &geometry, &committed)
            };
            if previous_drag.is_some() && geometry.dragged.is_none() {
                // Discard inventory taken before the owner's final mouse-up
                // transaction before reconciling its accepted dimensions.
                inventory.invalidate();
            }
            if !options.dry_run
                && engine.settings.gesture_scroll
                && input.is_none()
                && start.duration_since(last_input_attempt) >= Duration::from_secs(5)
            {
                last_input_attempt = start;
                // Tap creation is the live OS authorization check; a cached
                // preflight false must not prevent a later permission grant.
                match ribbon_macos::input::InputSource::start(&engine.settings) {
                    Ok(source) => input = Some(source),
                    Err(e) => eprintln!("Gestures unavailable: {e:#}"),
                }
            }
            if let Some(source) = &mut input {
                let blocked = gesture_blocked(&options, &mut gesture_frontmost);
                if let Err(e) = source.update(&engine.settings, gesture_targets(&engine, blocked)) {
                    eprintln!("Gestures suspended: {e:#}");
                    input = None;
                }
            }
            // At most eight clients per frame. Each client's total read deadline is bounded.
            for _ in 0..8 {
                match socket.listener.accept() {
                    Ok((mut stream, _)) => {
                        let response = ipc::receive(&mut stream)
                            .and_then(|r| {
                                if suspended && !matches!(r, Request::Status {} | Request::Quit {})
                                {
                                    bail!("Window operations are paused during Mission Control");
                                }
                                if geometry.dragged.is_some()
                                    && !matches!(r, Request::Status {} | Request::Quit {})
                                {
                                    bail!("Window operations are paused during a mouse drag");
                                }
                                let native_geometry = matches!(r, Request::SetMode { .. });
                                if native_geometry {
                                    inventory.invalidate();
                                }
                                handle(
                                    &mut engine,
                                    r,
                                    options.dry_run,
                                    &mut geometry,
                                    backend.as_ref(),
                                    &mut sizes,
                                    &mut quit,
                                )
                            })
                            .unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
                        if let Err(e) = ipc::respond(&mut stream, &response) {
                            eprintln!("IPC: {e:#}");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
            if quit {
                break;
            }
            if suspended {
                if let Some(backend) = &backend {
                    backend.heartbeat()?;
                }
                last_tick = Instant::now();
                std::thread::sleep(frame_duration);
                continue 'frames;
            }
            let after_ipc = Instant::now();
            let animating = engine.monitors.values().any(|m| {
                let scroll = &m.layout().scroll;
                !m.suspended && (scroll.position - scroll.target).abs() > 0.01
            }) || input
                .as_ref()
                .is_some_and(ribbon_macos::input::InputSource::active);
            if event_trace && notifications != 0 {
                eprintln!("Native events: {notifications}");
            }
            if (notifications & ribbon_macos::EventSource::APPS != 0
                || (!animating
                    && start.duration_since(last_watch_refresh) >= Duration::from_secs(1)))
                && geometry.dragged.is_none()
                && let Some(source) = &events
            {
                refresh_observers(source, &mut watching, &options)?;
                last_watch_refresh = start;
            }
            let after_events = Instant::now();
            if notifications
                & (ribbon_macos::EventSource::WINDOWS
                    | ribbon_macos::EventSource::GEOMETRY
                    | ribbon_macos::EventSource::APPS
                    | ribbon_macos::EventSource::FOCUS)
                != 0
                || start.duration_since(last_inventory) >= Duration::from_millis(100)
            {
                let next = ribbon_macos::displays()?;
                let signature = |list: &[Display]| {
                    let mut s: Vec<_> = list
                        .iter()
                        .map(|d| {
                            (
                                d.id.clone(),
                                d.viewport.x.to_bits(),
                                d.viewport.y.to_bits(),
                                d.viewport.width.to_bits(),
                                d.viewport.height.to_bits(),
                                d.scale.to_bits(),
                            )
                        })
                        .collect();
                    s.sort();
                    s
                };
                if !options.dry_run && signature(&next) != signature(&displays) {
                    bail!(
                        "Display geometry changed; releasing windows. Restart run to use the new display geometry"
                    );
                }
                if next.iter().any(|d| {
                    engine.monitors.get(&d.id).is_some_and(|m| {
                        m.native_space != d.native_space || m.suspended != d.native_fullscreen
                    })
                }) {
                    inventory.invalidate();
                }
                displays = next;
                // Activate the native context before processing focus/destruction,
                // without waiting for asynchronous window metadata.
                update_contexts(&mut engine, &displays)?;
                inventory.request()?;
                last_inventory = start;
            }
            if let Some(source) = &events {
                for closed in source.closed_windows() {
                    let id = WindowId(closed.wid);
                    // Some apps expose AXWindows only on their active Space.
                    // A withdrawn-list entry does not prove an inactive window closed.
                    if closed.withdrawn != 0 && inactive_window(&engine, &geometry, id) {
                        continue;
                    }
                    if geometry
                        .originals
                        .get(&id)
                        .is_some_and(|w| w.pid == closed.pid)
                    {
                        forget_closed(&mut engine, &mut geometry, id)?;
                        sizes.remove(&id);
                    }
                }
            }
            if let Some(windows) = inventory.poll()? {
                let reveal = !input
                    .as_ref()
                    .is_some_and(ribbon_macos::input::InputSource::active);
                synchronize(
                    &mut engine,
                    &mut geometry,
                    &options,
                    &displays,
                    &mut sizes,
                    (windows, reveal),
                )?;
            }
            if geometry.dragged.is_none()
                && (notifications & ribbon_macos::EventSource::FOCUS != 0
                    || start.duration_since(last_focus) >= Duration::from_millis(100))
            {
                let reveal = !input
                    .as_ref()
                    .is_some_and(ribbon_macos::input::InputSource::active);
                observe_native_focus(&mut engine, &geometry, &options, reveal);
                last_focus = start;
            }
            let after_inventory = Instant::now();
            if let Some(source) = &mut input {
                // A native Space notification may have changed targets in this batch.
                let blocked = gesture_blocked(&options, &mut gesture_frontmost);
                if let Err(e) = source.update(&engine.settings, gesture_targets(&engine, blocked)) {
                    eprintln!("Gestures suspended: {e:#}");
                    source.cancel();
                }
                if ribbon_macos::left_mouse_down() {
                    source.cancel();
                }
                for delta in source.drain() {
                    if engine
                        .monitors
                        .get(&delta.monitor)
                        .is_some_and(|m| !m.suspended && m.native_space == delta.space)
                    {
                        // No focus call, spring or synthesized momentum on this route.
                        engine.apply(&delta.monitor, &Action::Scroll { delta: delta.delta })?;
                    }
                }
            }
            // Do not recenter underneath the pointer during a border drag. Keep
            // adopting native sizes, then animate the final target after release.
            if geometry.dragged.is_none() && !ribbon_macos::left_mouse_down() {
                engine.tick(start.duration_since(last_tick).as_secs_f64());
            }
            last_tick = start;
            let before_backend = Instant::now();
            if let Some(backend) = &backend {
                for id in geometry.originals.keys().copied().collect::<Vec<_>>() {
                    if geometry
                        .originals
                        .get(&id)
                        .is_some_and(|w| ribbon_macos::window_owner(id) != w.pid)
                    {
                        forget_closed(&mut engine, &mut geometry, id)?;
                        sizes.remove(&id);
                    }
                }
                sizes.retain(|id, _| geometry.originals.contains_key(id));
                let owners = geometry
                    .originals
                    .iter()
                    .map(|(id, w)| (*id, w.pid))
                    .collect();
                let plans = engine.placements();
                if let Some(window) = geometry.dragged {
                    // Owner-driven drag loops change the transform themselves. Keep
                    // the lease and monitor clip, but never overwrite that transform.
                    let viewports = engine
                        .monitors
                        .iter()
                        .map(|(id, m)| (id.clone(), m.viewport))
                        .collect();
                    backend.interactive_frame(
                        &plans,
                        &owners,
                        &sticky_leases(&geometry),
                        &viewports,
                        window,
                        mouse.frame(
                            engine.monitors[&plans
                                .iter()
                                .find(|p| p.window == window)
                                .context("Missing drag placement")?
                                .monitor]
                                .viewport,
                        ),
                    )?;
                    // AppKit translates its current compositor transform for
                    // every native move. Repeated absolute pointer corrections
                    // feed the previous correction back into the next owner
                    // transaction, making the two writers fight. Remove the
                    // initial physical/presentation offset once, then let the
                    // owner move while the backend updates only the clip.
                    mouse.corrected |= mouse.changed;
                    last_tick = Instant::now();
                    last_frame.clear();
                    std::thread::sleep(frame_duration);
                    continue 'frames;
                }
                if ribbon_macos::left_mouse_down() {
                    // Content clicks and unmanaged drags must not let an AX
                    // resize/rebase run before capture has identified chrome.
                    backend.heartbeat()?;
                    last_tick = Instant::now();
                    std::thread::sleep(frame_duration);
                    continue 'frames;
                }
                // Native geometry has a size-only request ledger. Scrolling,
                // focus and mouse release change compositor placement, never
                // erase accepted dimensions to trigger an AX position rebase.
                let mut geometry_changed = false;
                // Hold the previously committed layout through the entire AX resize
                // batch. Do not publish half of a newly split column between owners.
                let pending = plans
                    .iter()
                    .filter(|p| {
                        engine
                            .monitors
                            .get(&p.monitor)
                            .is_some_and(|m| !m.suspended && m.native_space == p.native_space)
                    })
                    .any(|p| {
                        geometry
                            .originals
                            .get(&p.window)
                            .is_some_and(|w| sizes.needs_resize(p.window, w.pid, p.frame))
                    });
                let mut held = plans.clone();
                if pending {
                    for p in &mut held {
                        let w = geometry
                            .originals
                            .get(&p.window)
                            .context("Missing original geometry")?;
                        if let Some(previous) = committed
                            .iter()
                            .find(|old| {
                                old.window == p.window && old.native_space == p.native_space
                            })
                            .filter(|_| sizes.get(&p.window).is_some_and(|s| s.0 == w.pid))
                        {
                            *p = previous.clone();
                        } else {
                            p.frame =
                                ribbon_macos::window_presentation(w.id, w.pid).unwrap_or(w.bounds);
                            p.clip = p.frame.intersection(engine.monitors[&p.monitor].viewport);
                        }
                    }
                    // Drag captures already took the interactive-only branch
                    // above. Save originals before AX changes the transform.
                    backend.frame_with_sticky_in_viewports(
                        &held,
                        &owners,
                        &sticky_leases(&geometry),
                        &monitor_viewports(&engine),
                    )?;
                }
                let mut lease_tick = Instant::now();
                for p in &plans {
                    let m = engine.monitors.get(&p.monitor).context("Missing monitor")?;
                    if m.suspended || m.native_space != p.native_space {
                        continue;
                    }
                    let w = geometry
                        .originals
                        .get(&p.window)
                        .context("Missing original geometry")?;
                    let size = (w.pid, p.frame.width, p.frame.height);
                    if sizes.needs_resize(p.window, w.pid, p.frame) {
                        // Record before writing: even a failed AX request may have partially changed geometry.
                        geometry.resized.insert(p.window);
                        let viewport = engine
                            .monitors
                            .get(&p.monitor)
                            .context("Missing monitor")?
                            .viewport;
                        sizes.requests = sizes.requests.saturating_add(1);
                        let resized = ribbon_macos::resize_window_observed(
                            p.window,
                            w.pid,
                            if p.clip == Some(p.frame) {
                                p.frame
                            } else {
                                resize_anchor(viewport, w.bounds, size.1, size.2)
                            },
                            || {
                                if ribbon_macos::left_mouse_down() {
                                    bail!("Native mouse interaction began during geometry update");
                                }
                                backend.frame_with_sticky_in_viewports(
                                    &held,
                                    &owners,
                                    &sticky_leases(&geometry),
                                    &monitor_viewports(&engine),
                                )
                            },
                        );
                        inventory.invalidate();
                        if let Err(e) = resized {
                            if ribbon_macos::left_mouse_down() {
                                last_frame.clear();
                                continue 'frames;
                            }
                            if ribbon_macos::window_owner(p.window) != w.pid {
                                forget_closed(&mut engine, &mut geometry, p.window)?;
                                sizes.remove(&p.window);
                                continue 'frames;
                            }
                            eprintln!(
                                "Leaving window {} floating after resize refusal: {e:#}",
                                p.window.0
                            );
                            if let Some(logical) = geometry.logical_originals.get(&p.window)
                                && let Err(restore) = ribbon_macos::restore_window(w, *logical)
                            {
                                eprintln!("Geometry restore after resize refusal: {restore:#}");
                            }
                            engine.remove_window(p.window)?;
                            geometry
                                .modes
                                .get_mut(&p.window)
                                .context("Missing window mode")?
                                .floating = true;
                            geometry.resized.remove(&p.window);
                            sizes.remove(&p.window);
                            let owners = geometry
                                .originals
                                .iter()
                                .map(|(id, w)| (*id, w.pid))
                                .collect();
                            backend.frame_with_sticky_in_viewports(
                                &engine.placements(),
                                &owners,
                                &sticky_leases(&geometry),
                                &monitor_viewports(&engine),
                            )?;
                            last_frame.clear();
                            committed.retain(|old| old.window != p.window);
                            continue 'frames;
                        }
                        sizes.insert(p.window, size);
                        geometry_changed = true;
                        // A slow owner must not expire the restoration lease. The
                        // heartbeat holds old placements until all owners are ready.
                        if lease_tick.elapsed() >= Duration::from_millis(250) {
                            backend.frame_with_sticky_in_viewports(
                                &held,
                                &owners,
                                &sticky_leases(&geometry),
                                &monitor_viewports(&engine),
                            )?;
                            lease_tick = Instant::now();
                        }
                    }
                }
                if geometry_changed {
                    // AX work can take several frames. Resume the spring from its
                    // displayed position rather than jumping over that stalled time.
                    last_tick = Instant::now();
                }
                // Focus/native selection metadata does not change compositor geometry.
                let frame = serde_json::to_vec(
                    &plans
                        .iter()
                        .map(|p| (p.window, p.frame, p.clip))
                        .collect::<Vec<_>>(),
                )?;
                if geometry_changed
                    || frame != last_frame
                    || start.duration_since(last_send) >= Duration::from_millis(50)
                {
                    backend.frame_with_sticky_in_viewports(
                        &plans,
                        &owners,
                        &sticky_leases(&geometry),
                        &monitor_viewports(&engine),
                    )?;
                    last_frame = frame;
                    committed = plans;
                    last_send = Instant::now();
                }
            }
            if frame_trace {
                eprintln!(
                    "Frame timing: {}",
                    json!({"ipc_ms":after_ipc.duration_since(start).as_secs_f64()*1000.0,
                "events_ms":after_events.duration_since(after_ipc).as_secs_f64()*1000.0,
                "inventory_ms":after_inventory.duration_since(after_events).as_secs_f64()*1000.0,
                "input_ms":before_backend.duration_since(after_inventory).as_secs_f64()*1000.0,
                "backend_ms":before_backend.elapsed().as_secs_f64()*1000.0,"notifications":notifications,"animating":animating})
                );
            }
            if let Some(remaining) = frame_duration.checked_sub(start.elapsed()) {
                std::thread::sleep(remaining);
            }
        }
        Ok(())
    })();
    if let Some(backend) = &backend {
        // An error path must not move another controller's windows while
        // launchd retries us. Cleanup needs ownership as much as placement.
        if let Err(error) = backend.heartbeat() {
            geometry.resized.clear();
            frame_result?;
            return Err(error.context("Cannot release geometry without the Dock lease"));
        }
        let owners = geometry
            .originals
            .iter()
            .map(|(id, w)| (*id, w.pid))
            .collect();
        let mut released = release_candidates(engine.placements(), &committed, &geometry.resized);
        let held = released.clone();
        for (index, p) in released.iter_mut().enumerate() {
            let Some(w) = geometry.originals.get(&p.window) else {
                continue;
            };
            if ribbon_macos::window_owner(p.window) != w.pid {
                continue;
            }
            let shown = ribbon_macos::window_presentation(p.window, w.pid).unwrap_or(p.frame);
            p.frame = released_frame(&engine, &p.monitor, shown, index);
            p.clip = Some(p.frame);
            if let Err(error) =
                ribbon_macos::resize_window_observed(p.window, w.pid, p.frame, || {
                    backend.frame_with_sticky_in_viewports(
                        &held,
                        &owners,
                        &sticky_leases(&geometry),
                        &monitor_viewports(&engine),
                    )
                })
            {
                eprintln!("Stop geometry for {}: {error:#}", p.window.0);
            }
        }
        backend.finish(&released, &owners)?;
        geometry.resized.clear();
    }
    frame_result?;
    Ok(())
}

fn gesture_blocked(options: &Options, cached: &mut (i32, bool)) -> bool {
    // Metadata-only exclusion, before AX. Protected apps never become an input
    // test surface or receive an intercepted gesture through this daemon.
    let pid = ribbon_macos::frontmost_pid();
    if cached.0 != pid {
        *cached = (
            pid,
            ribbon_macos::applications().map_or(true, |apps| {
                apps.iter().any(|a| {
                    a.pid == pid && identity_excluded(&a.app, &a.bundle_id, &options.exclude_apps)
                })
            }),
        );
    }
    cached.1
}
fn gesture_targets(engine: &Engine, blocked: bool) -> Vec<ribbon_macos::input::Target> {
    if blocked || ribbon_macos::left_mouse_down() {
        return Vec::new();
    }
    engine
        .monitors
        .values()
        .filter(|m| !m.suspended && !m.layout().columns.is_empty())
        .map(|m| ribbon_macos::input::Target {
            monitor: m.id.clone(),
            space: m.native_space,
            viewport: m.viewport,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbon_core::{NativeSpaceId, Rect};

    #[test]
    fn failed_initial_frame_never_releases_unowned_geometry() {
        let mut engine = Engine::new(Settings::default()).unwrap();
        engine
            .update_monitor(
                "test",
                Rect {
                    x: 0.0,
                    y: 30.0,
                    width: 2560.0,
                    height: 1410.0,
                },
                NativeSpaceId(1),
                false,
            )
            .unwrap();
        for id in 1..=3 {
            engine.add_window("test", WindowId(id), None).unwrap();
        }
        let plans = engine.placements();
        // The first backend frame was rejected before any geometry write.
        assert!(release_candidates(plans.clone(), &[], &BTreeSet::new()).is_empty());
        // A later failure releases a committed tile and a partial AX write,
        // but not a newly discovered window that was never controlled.
        let released =
            release_candidates(plans.clone(), &plans[..1], &BTreeSet::from([WindowId(2)]));
        assert_eq!(
            released.iter().map(|p| p.window).collect::<Vec<_>>(),
            vec![WindowId(1), WindowId(2)]
        );
    }

    #[test]
    fn native_drag_gets_one_offset_correction_then_yields_to_its_owner() {
        let shown = Rect {
            x: 24.0,
            y: 57.0,
            width: 1464.0,
            height: 901.0,
        };
        let viewport = Rect {
            x: 0.0,
            y: 33.0,
            width: 1512.0,
            height: 949.0,
        };
        let mut capture = MouseCapture {
            point: (174.0, 88.0),
            shown: Some(shown),
            current: Some(shown),
            changed: true,
            ..MouseCapture::default()
        };
        let first = capture.frame_for_pointer((179.0, 91.0), viewport).unwrap();
        assert_eq!((first.x, first.y), (29.0, 60.0));
        capture.corrected = true;
        // Owner moves are no longer overwritten by a newer pointer sample.
        capture.current = Some(Rect {
            x: 210.0,
            y: 150.0,
            ..shown
        });
        assert!(
            capture
                .frame_for_pointer((420.0, 250.0), viewport)
                .is_none()
        );
        // Release creates a fresh capture; correction applies again if needed.
        capture.corrected = false;
        assert_eq!(
            capture.frame_for_pointer((179.0, 0.0), viewport).unwrap().y,
            33.0
        );
    }

    #[test]
    fn pointer_drag_does_not_double_logical_translation_offsets() {
        let start = Rect {
            x: 301.0,
            y: 57.0,
            width: 910.0,
            height: 901.0,
        };
        // The owner relocated its physical frame from x100 to x312 and
        // WindowServer applied the old presentation offset again: x513.
        let doubled = Rect {
            x: 513.0,
            y: 39.0,
            ..start
        };
        let corrected = drag_frame(
            start,
            (756.0, 67.0),
            (767.0, 73.0),
            doubled,
            false,
            false,
            false,
        );
        assert_eq!(corrected.x, 312.0);
        assert_eq!(corrected.y, 63.0);
        // Right/bottom resizing keeps the opposite visual corner fixed,
        // even if the owner changes its logical anchor at acquisition.
        let resized = Rect {
            width: 1020.0,
            height: 850.0,
            ..doubled
        };
        let right = drag_frame(
            start,
            (1210.0, 400.0),
            (1320.0, 400.0),
            resized,
            true,
            false,
            false,
        );
        assert_eq!((right.x, right.y), (301.0, 57.0));
        let upper_left = drag_frame(
            start,
            (302.0, 58.0),
            (192.0, 109.0),
            resized,
            true,
            true,
            true,
        );
        assert_eq!(
            (
                upper_left.x + upper_left.width,
                upper_left.y + upper_left.height
            ),
            (start.x + start.width, start.y + start.height)
        );
    }

    #[test]
    fn mouse_down_target_survives_delayed_app_activation() {
        let frame = Rect {
            x: 100.0,
            y: 57.0,
            width: 800.0,
            height: 600.0,
        };
        let id = WindowId(42);
        let plans = vec![Placement {
            window: id,
            monitor: "main".into(),
            native_space: NativeSpaceId(3),
            frame,
            clip: Some(frame),
            focused: false,
        }];
        let mut geometry = GeometryLease {
            modes: BTreeMap::new(),
            originals: BTreeMap::new(),
            logical_originals: BTreeMap::new(),
            resized: BTreeSet::new(),
            dragged: None,
        };
        geometry.originals.insert(
            id,
            Window {
                id,
                pid: 123,
                app: "Owned fixture".into(),
                bundle_id: "test.fixture".into(),
                title: String::new(),
                layer: 0,
                onscreen: true,
                bounds: frame,
                surface_bounds: Some(frame),
                native_spaces: vec![NativeSpaceId(3)],
                sticky: false,
                sticky_known: true,
            },
        );
        let press = (200.0, 65.0);
        // The target is owned by a different app from the current frontmost app.
        assert_eq!(
            pressed_placement(&plans, &geometry, 42, 999, press)
                .unwrap()
                .window,
            id
        );
        // Without an event window, wait for activation and retry the same press.
        assert!(pressed_placement(&plans, &geometry, 0, 999, press).is_none());
        assert_eq!(
            pressed_placement(&plans, &geometry, 0, 123, press)
                .unwrap()
                .window,
            id
        );
        // Clicking an unmanaged overlay must never grab a managed window beneath it.
        assert!(pressed_placement(&plans, &geometry, 777, 123, press).is_none());
        // Hidden columns do not acquire a mouse capture.
        let mut hidden = plans;
        hidden[0].clip = None;
        assert!(pressed_placement(&hidden, &geometry, 42, 123, press).is_none());
    }

    #[test]
    fn withdrawn_windows_remain_known_on_inactive_spaces_for_tiles_and_floats() {
        let viewport = Rect {
            x: 0.0,
            y: 33.0,
            width: 1512.0,
            height: 949.0,
        };
        let mut engine = Engine::new(Settings::default()).unwrap();
        engine
            .update_monitor("main", viewport, NativeSpaceId(3), false)
            .unwrap();
        let id = WindowId(1);
        engine.add_window("main", id, Some(800.0)).unwrap();
        let mut geometry = GeometryLease {
            modes: BTreeMap::new(),
            originals: BTreeMap::new(),
            logical_originals: BTreeMap::new(),
            resized: BTreeSet::new(),
            dragged: None,
        };
        geometry.originals.insert(
            id,
            Window {
                id,
                pid: 1,
                app: "Owned test".into(),
                bundle_id: String::new(),
                title: String::new(),
                layer: 0,
                onscreen: false,
                bounds: viewport,
                surface_bounds: Some(viewport),
                native_spaces: vec![NativeSpaceId(3)],
                sticky: false,
                sticky_known: true,
            },
        );
        assert!(!inactive_window(&engine, &geometry, id));
        engine
            .update_monitor("main", viewport, NativeSpaceId(7), false)
            .unwrap();
        assert!(inactive_window(&engine, &geometry, id));
        engine.remove_window(id).unwrap();
        geometry.modes.insert(
            id,
            WindowMode {
                floating: true,
                sticky: false,
                ..WindowMode::default()
            },
        );
        assert!(inactive_window(&engine, &geometry, id));
        geometry.modes.get_mut(&id).unwrap().sticky = true;
        assert!(!inactive_window(&engine, &geometry, id));
        geometry.modes.get_mut(&id).unwrap().sticky = false;
        engine
            .update_monitor("main", viewport, NativeSpaceId(3), false)
            .unwrap();
        assert!(!inactive_window(&engine, &geometry, id));
    }

    #[test]
    fn inventory_does_not_adopt_geometry_sampled_before_a_completed_ax_write() {
        let (request, requests) = mpsc::sync_channel(1);
        let (results, result) = mpsc::channel();
        let mut inventory = WindowInventory {
            request: Some(request),
            result,
            worker: None,
            pending: false,
            generation: 0,
            dirty: false,
            known: Arc::new(Mutex::new(BTreeMap::new())),
        };
        inventory.request().unwrap();
        let old = requests.recv().unwrap();
        inventory.invalidate(); // A WM resize completes while CG is reading.
        results
            .send((old, Ok(InventorySnapshot::default())))
            .unwrap();
        assert!(inventory.poll().unwrap().is_none());
        let current = requests.recv().unwrap();
        assert_ne!(old, current);
        results
            .send((current, Ok(InventorySnapshot::default())))
            .unwrap();
        assert!(inventory.poll().unwrap().is_some());

        inventory.request().unwrap();
        let pending = requests.recv().unwrap();
        inventory.request().unwrap(); // A creation event arrives during a query.
        inventory.request().unwrap();
        assert!(requests.try_recv().is_err());
        results
            .send((pending, Ok(InventorySnapshot::default())))
            .unwrap();
        assert!(inventory.poll().unwrap().is_some());
        assert_eq!(requests.recv().unwrap(), pending);
        assert!(requests.try_recv().is_err());

        // A transient native JSON/query failure must not stop the controller
        // or replace its layout with an empty inventory.
        results
            .send((pending, Err(anyhow::anyhow!("invalid native rectangle"))))
            .unwrap();
        assert!(inventory.poll().unwrap().is_none());
        assert!(!inventory.pending);
        assert!(requests.try_recv().is_err());
        inventory.request().unwrap();
        assert_eq!(requests.recv().unwrap(), pending);
        results
            .send((pending, Ok(InventorySnapshot::default())))
            .unwrap();
        assert!(inventory.poll().unwrap().is_some());
    }

    #[test]
    fn resize_anchor_preserves_valid_secondary_menu_bar_offsets_and_bounds_large_resizes() {
        let right = Rect {
            x: 2560.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
        let slack = Rect {
            x: 2560.0,
            y: 30.0,
            width: 2560.0,
            height: 1410.0,
        };
        assert_eq!(
            resize_anchor(right, slack, 2520.0, 1400.0),
            Rect {
                x: 2560.0,
                y: 30.0,
                width: 2520.0,
                height: 1400.0
            }
        );
        let primary = Rect {
            x: 0.0,
            y: 30.0,
            width: 2560.0,
            height: 1410.0,
        };
        let terminal = Rect {
            x: 880.0,
            y: 233.0,
            width: 800.0,
            height: 600.0,
        };
        let anchor = resize_anchor(primary, terminal, 800.0, 1370.0);
        assert_eq!(anchor.y, 70.0);
        assert_eq!(anchor.intersection(primary), Some(anchor));
        let builtin = Rect {
            x: -1187.0,
            y: 1472.0,
            width: 1512.0,
            height: 950.0,
        };
        let anchor = resize_anchor(builtin, terminal, 640.0, 910.0);
        assert_eq!(anchor.x, -315.0);
        assert_eq!(anchor.y, 1472.0);
        assert_eq!(anchor.intersection(builtin), Some(anchor));
    }

    #[test]
    fn discovery_rejects_display_helpers_and_excluded_apps() {
        let mut window = Window {
            id: WindowId(1),
            pid: 1,
            app: "Alacritty".into(),
            bundle_id: "org.alacritty".into(),
            title: String::new(),
            layer: 0,
            onscreen: true,
            bounds: Rect {
                x: 1381.0,
                y: 378.0,
                width: 729.0,
                height: 900.0,
            },
            surface_bounds: None,
            native_spaces: vec![NativeSpaceId(3)],
            sticky: false,
            sticky_known: true,
        };
        let exclusions = vec!["com.openai.*".into(), "ChatGPT*".into()];
        assert!(normal_window_candidate(&window) && !app_excluded(&window, &exclusions));
        let options = Options {
            all: true,
            selected: Vec::new(),
            dry_run: false,
            exclude_apps: exclusions.clone(),
            settings: Settings::default(),
        };
        let unknown = InventorySnapshot::probe(
            vec![window.clone()],
            &options,
            &BTreeMap::new(),
            |pid, ids| {
                assert_eq!(pid, 1);
                assert_eq!(ids, &[1]);
                bail!("owner unresponsive")
            },
        );
        assert!(unknown.members.is_empty()); // Timeout must not mean every window closed.
        assert!(unknown.ready.is_empty());
        let physical = window.bounds;
        window.surface_bounds = Some(physical);
        window.bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 105.0,
            height: 600.0,
        };
        assert_eq!(window_bounds(&window), physical);
        window.bounds = Rect {
            x: 19.0,
            y: 124.0,
            width: 317.0,
            height: 390.0,
        };
        assert_eq!(window_bounds(&window), physical);
        // An unavailable optional surface query retains the legacy fallback.
        window.surface_bounds = Some(Rect {
            width: 0.0,
            ..physical
        });
        assert_eq!(window_bounds(&window), window.bounds);
        window.surface_bounds = None;
        window.app = "BetterDisplay".into();
        window.bundle_id = "pro.betterdisplay.BetterDisplay".into();
        window.bounds = Rect {
            x: 0.0,
            y: 1439.0,
            width: 1.0,
            height: 1.0,
        };
        assert!(!normal_window_candidate(&window));
        window.app = "ChatGPT".into();
        window.bundle_id = "com.openai.codex".into();
        window.bounds.width = 1280.0;
        window.bounds.height = 1410.0;
        assert!(normal_window_candidate(&window) && app_excluded(&window, &exclusions));
        assert!(app_excluded(&window, &["com.openai.codex".into()]));
        assert!(!app_excluded(&window, &["com.openai.chat".into()]));
        let protected =
            InventorySnapshot::probe(vec![window], &options, &BTreeMap::new(), |_, _| {
                panic!("excluded application must never receive an AX probe")
            });
        assert!(protected.members.is_empty());
    }

    #[test]
    fn leaving_management_preserves_visible_positions_and_recovers_hidden_columns() {
        let settings = Settings {
            padding_top: Some(24.0),
            padding_bottom: Some(24.0),
            padding_left: Some(24.0),
            padding_right: Some(24.0),
            ..Settings::default()
        };
        let mut engine = Engine::new(settings).unwrap();
        engine
            .update_monitor(
                "main",
                Rect {
                    x: 0.0,
                    y: 33.0,
                    width: 1512.0,
                    height: 949.0,
                },
                NativeSpaceId(3),
                false,
            )
            .unwrap();
        let visible = Rect {
            x: 298.0,
            y: 57.0,
            width: 732.0,
            height: 901.0,
        };
        assert_eq!(released_frame(&engine, "main", visible, 0), visible);
        let hidden = Rect {
            x: 2400.0,
            ..visible
        };
        let recovered = released_frame(&engine, "main", hidden, 2);
        assert_eq!(recovered.x, 72.0);
        assert_eq!(recovered.y, 57.0);
        assert_eq!(recovered.width, 732.0);
        let partial = Rect {
            x: -310.0,
            ..visible
        };
        assert_eq!(released_frame(&engine, "main", partial, 0).x, 24.0);
        assert!(!chrome_hit(visible, (500.0, 400.0))); // Ordinary content selection.
        assert!(chrome_hit(visible, (500.0, 65.0))); // Title bar.
        assert!(chrome_hit(visible, (1028.0, 400.0))); // Resize border.
        engine
            .update_monitor(
                "above",
                Rect {
                    x: -1600.0,
                    y: -867.0,
                    width: 1600.0,
                    height: 867.0,
                },
                NativeSpaceId(7),
                false,
            )
            .unwrap();
        let upper = released_frame(
            &engine,
            "above",
            Rect {
                x: -1500.0,
                y: -840.0,
                width: 800.0,
                height: 600.0,
            },
            0,
        );
        assert_eq!(upper.y, -840.0); // Never clamp another monitor to global y=0.
    }
}
