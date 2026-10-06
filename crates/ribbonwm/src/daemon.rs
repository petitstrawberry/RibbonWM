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
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

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
}
/// Read-only WindowServer metadata runs away from the frame loop. AX, AppKit
/// notifications, policy and compositor commits stay on the controlling thread.
struct WindowInventory {
    request: Option<mpsc::SyncSender<u64>>,
    result: mpsc::Receiver<(u64, Result<Vec<Window>>)>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: bool,
    generation: u64,
    dirty: bool,
}
impl WindowInventory {
    fn start() -> Self {
        let (request, requests) = mpsc::sync_channel(1);
        let (results, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            while let Ok(generation) = requests.recv() {
                if results.send((generation, ribbon_macos::windows())).is_err() {
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
        }
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
    fn poll(&mut self) -> Result<Option<Vec<Window>>> {
        let snapshot = match self.result.try_recv() {
            Ok((generation, result)) => {
                self.pending = false;
                if generation == self.generation {
                    Some(result?)
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
        let _ = engine.observe_focus(id, reveal);
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
    displays
        .iter()
        .filter(|d| {
            !d.native_fullscreen && (window.sticky || window.native_spaces == vec![d.native_space])
        })
        .max_by(|a, b| {
            let area = |d: &Display| {
                window
                    .bounds
                    .intersection(d.frame)
                    .map_or(0.0, |r| r.width * r.height)
            };
            area(a).total_cmp(&area(b))
        })
        .filter(|d| window.bounds.intersection(d.frame).is_some())
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
    // Display utilities may expose 1x1 normal-layer helper surfaces. They
    // are not user app windows and must never be expanded into a column.
    window.layer == 0
        && window.bounds.valid()
        && window.bounds.width >= 100.0
        && window.bounds.height >= 100.0
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
fn synchronize(
    engine: &mut Engine,
    geometry: &mut GeometryLease,
    options: &Options,
    displays: &[Display],
    sizes: &mut BTreeMap<WindowId, (i32, f64, f64)>,
    snapshot: (Vec<Window>, bool),
) -> Result<()> {
    let (mut inventory, reveal_focus) = snapshot;
    // Only a cached destination has a saved offset; initial discovery should
    // reveal the native active window normally.
    let context_changed = displays.iter().any(|d| {
        engine.monitors.get(&d.id).is_some_and(|m| {
            (m.native_space != d.native_space && m.contexts.contains_key(&d.native_space))
                || m.suspended != d.native_fullscreen
        })
    });
    for d in displays {
        engine.update_monitor(&d.id, d.viewport, d.native_space, d.native_fullscreen)?;
    }
    for m in engine.monitors.values_mut() {
        if !displays.iter().any(|d| d.id == m.id) {
            m.suspended = true;
        }
    }
    for id in geometry.originals.keys().copied().collect::<Vec<_>>() {
        let old = geometry.originals.get(&id);
        if !inventory
            .iter()
            .any(|w| w.id == id && old.is_none_or(|o| o.pid == w.pid))
        {
            forget_closed(engine, geometry, id)?;
        }
    }
    observe_native_focus(engine, geometry, options, reveal_focus && !context_changed);
    // CG inventories are in stacking order; window IDs give deterministic
    // creation order when several windows arrive between discovery polls.
    inventory.sort_by_key(|w| w.id);
    for w in inventory {
        if app_excluded(&w, &options.exclude_apps) {
            continue;
        }
        if geometry.modes.get(&w.id).is_some_and(|m| !m.wants_tile()) {
            continue;
        }
        if let Some((monitor, space)) = engine.window_context(w.id) {
            if let Some(display) = display_for(&w, displays)
                && (options.dry_run || !ribbon_macos::left_mouse_down())
                && (monitor != display.id || space != display.native_space)
            {
                engine.relocate_window(&display.id, w.id)?;
            }
            if !options.dry_run
                && w.onscreen
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
                        && (w.bounds.width - width).abs() > 2.0
                        && (100.0..=10000.0).contains(&w.bounds.width)
                    {
                        engine.observe_width(w.id, w.bounds.width)?;
                        accepted.1 = w.bounds.width;
                    }
                    if (plan.frame.height - height).abs() <= 2.0
                        && (w.bounds.height - height).abs() > 2.0
                        && w.bounds.height >= 100.0
                    {
                        let _ = engine.resize_row(&plan.monitor, w.id, w.bounds.height);
                        accepted.2 = w.bounds.height;
                    }
                    sizes.insert(w.id, accepted);
                }
            }
            continue;
        }
        if !normal_window_candidate(&w) {
            continue;
        }
        if !options.all && !options.selected.contains(&w.id.0) {
            continue;
        }
        let Some(display) = display_for(&w, displays) else {
            continue;
        };
        if !options.dry_run && !ribbon_macos::window_manageable(w.id, w.pid) {
            continue;
        }
        if !options.dry_run {
            // The presented CG rectangle can differ from the owner's AX frame.
            // Save before Dock or AX writes; restore each through its own API.
            let Ok(logical) = ribbon_macos::window_geometry(w.id, w.pid) else {
                continue;
            };
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
    sizes: &mut BTreeMap<WindowId, (i32, f64, f64)>,
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
            if !dry_run && geometry.resized.contains(&id) {
                ribbon_macos::restore_window(
                    &original,
                    old_logical.context("Missing original AX geometry")?,
                )?;
            }
            engine.remove_window(id)?;
        } else if next.wants_tile() && !engine.window_ids().contains(&id) {
            let current = ribbon_macos::windows()?
                .into_iter()
                .find(|w| w.id == id && w.pid == original.pid)
                .context("Window disappeared")?;
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
            backend.frame_with_sticky(&engine.placements(), &owners, &sticky_leases(geometry))?;
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
    sizes: &mut BTreeMap<WindowId, (i32, f64, f64)>,
    quit: &mut bool,
) -> Result<serde_json::Value> {
    match request {
        Request::Status {} => {
            let pid = ribbon_macos::frontmost_pid();
            let native_focused_window = (!dry_run
                && geometry.originals.values().any(|w| w.pid == pid))
            .then(|| ribbon_macos::focused_window(pid))
            .flatten();
            let original_geometry: BTreeMap<_, _> = geometry
                .originals
                .iter()
                .map(|(id, w)| (*id, json!({"pid":w.pid,"bounds":w.bounds})))
                .collect();
            return Ok(
                json!({"ok":true,"mode":if dry_run{"dry_run"}else{"live"},"state":engine,"original_geometry":original_geometry,"window_modes":geometry.modes,"native_focused_window":native_focused_window,"placements":engine.placements()}),
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
        Some(backend)
    };
    let mut geometry = GeometryLease {
        modes: BTreeMap::new(),
        originals: BTreeMap::new(),
        logical_originals: BTreeMap::new(),
        resized: BTreeSet::new(),
    };
    let mut engine = Engine::new(options.settings.clone())?;
    let mut displays = ribbon_macos::displays()?;
    if displays.is_empty() || displays.iter().any(|d| d.native_space.0 == 0) {
        bail!("Could not resolve current native Space context");
    }
    let mut sizes: BTreeMap<WindowId, (i32, f64, f64)> = BTreeMap::new();
    let events = (!options.dry_run).then(ribbon_macos::EventSource::default);
    let mut watching = BTreeSet::new();
    if let Some(source) = &events {
        refresh_observers(source, &mut watching, &options)?;
    }
    synchronize(
        &mut engine,
        &mut geometry,
        &options,
        &displays,
        &mut sizes,
        (ribbon_macos::windows()?, true),
    )?;
    for requested in &options.selected {
        if !geometry.originals.contains_key(&WindowId(*requested)) {
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
    let mut input = None;
    let mut last_input_attempt = last_tick - Duration::from_secs(5);
    let mut gesture_frontmost = (-1, true);
    let mut inventory = WindowInventory::start();
    eprintln!(
        "RibbonWM {}: {} windows; socket {}",
        if options.dry_run { "dry run" } else { "live" },
        engine.window_ids().len(),
        ipc::socket_path().display()
    );
    'frames: while running.load(Ordering::Relaxed) && !quit {
        let start = Instant::now();
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
        let after_ipc = Instant::now();
        let animating = engine.monitors.values().any(|m| {
            let scroll = &m.layout().scroll;
            !m.suspended && (scroll.position - scroll.target).abs() > 0.01
        }) || input
            .as_ref()
            .is_some_and(ribbon_macos::input::InputSource::active);
        let notifications = events.as_ref().map_or(0, ribbon_macos::EventSource::drain);
        if let Some(source) = &events {
            for closed in source.closed_windows(!animating) {
                let id = WindowId(closed.wid);
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
        if event_trace && notifications != 0 {
            eprintln!("Native events: {notifications}");
        }
        if (notifications & ribbon_macos::EventSource::APPS != 0
            || (!animating && start.duration_since(last_watch_refresh) >= Duration::from_secs(1)))
            && let Some(source) = &events
        {
            refresh_observers(source, &mut watching, &options)?;
            last_watch_refresh = start;
        }
        let after_events = Instant::now();
        if notifications
            & (ribbon_macos::EventSource::WINDOWS
                | ribbon_macos::EventSource::GEOMETRY
                | ribbon_macos::EventSource::APPS)
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
            displays = next;
            inventory.request()?;
            last_inventory = start;
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
        if notifications & ribbon_macos::EventSource::FOCUS != 0
            || start.duration_since(last_focus) >= Duration::from_millis(100)
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
        if options.dry_run || !ribbon_macos::left_mouse_down() {
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
            if ribbon_macos::left_mouse_down() {
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
                )?;
                last_tick = Instant::now();
                last_frame.clear();
                std::thread::sleep(frame_duration);
                continue 'frames;
            }
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
                    geometry.originals.get(&p.window).is_some_and(|w| {
                        sizes.get(&p.window) != Some(&(w.pid, p.frame.width, p.frame.height))
                    })
                });
            let mut held = plans.clone();
            if pending {
                let dragging = ribbon_macos::left_mouse_down();
                for p in &mut held {
                    let w = geometry
                        .originals
                        .get(&p.window)
                        .context("Missing original geometry")?;
                    if let Some(previous) = committed
                        .iter()
                        .find(|old| old.window == p.window && old.native_space == p.native_space)
                        .filter(|_| sizes.get(&p.window).is_some_and(|s| s.0 == w.pid))
                    {
                        *p = previous.clone();
                    } else {
                        p.frame = w.bounds;
                        p.clip = Some(w.bounds);
                    }
                }
                // Also saves originals before AX changes the owner's transform.
                // Do not send AX size writes into an in-progress mouse drag.
                // Keep the compositor lease alive and reconsider on release.
                if dragging {
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
                    )?;
                    last_tick = Instant::now();
                    std::thread::sleep(frame_duration);
                    continue 'frames;
                }
                backend.frame_with_sticky(&held, &owners, &sticky_leases(&geometry))?;
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
                if sizes.get(&p.window) != Some(&size) {
                    // Record before writing: even a failed AX request may have partially changed geometry.
                    geometry.resized.insert(p.window);
                    let viewport = engine
                        .monitors
                        .get(&p.monitor)
                        .context("Missing monitor")?
                        .viewport;
                    let resized = ribbon_macos::resize_window_observed(
                        p.window,
                        w.pid,
                        resize_anchor(viewport, w.bounds, size.1, size.2),
                        || backend.frame_with_sticky(&held, &owners, &sticky_leases(&geometry)),
                    );
                    inventory.invalidate();
                    if let Err(e) = resized {
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
                        backend.frame_with_sticky(
                            &engine.placements(),
                            &owners,
                            &sticky_leases(&geometry),
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
                        backend.frame_with_sticky(&held, &owners, &sticky_leases(&geometry))?;
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
                backend.frame_with_sticky(&plans, &owners, &sticky_leases(&geometry))?;
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
        };
        inventory.request().unwrap();
        let old = requests.recv().unwrap();
        inventory.invalidate(); // A WM resize completes while CG is reading.
        results.send((old, Ok(Vec::new()))).unwrap();
        assert!(inventory.poll().unwrap().is_none());
        let current = requests.recv().unwrap();
        assert_ne!(old, current);
        results.send((current, Ok(Vec::new()))).unwrap();
        assert!(inventory.poll().unwrap().is_some());

        inventory.request().unwrap();
        let pending = requests.recv().unwrap();
        inventory.request().unwrap(); // A creation event arrives during a query.
        inventory.request().unwrap();
        assert!(requests.try_recv().is_err());
        results.send((pending, Ok(Vec::new()))).unwrap();
        assert!(inventory.poll().unwrap().is_some());
        assert_eq!(requests.recv().unwrap(), pending);
        assert!(requests.try_recv().is_err());
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
            native_spaces: vec![NativeSpaceId(3)],
            sticky: false,
            sticky_known: true,
        };
        let exclusions = vec!["com.openai.*".into(), "ChatGPT*".into()];
        assert!(normal_window_candidate(&window) && !app_excluded(&window, &exclusions));
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
    }
}
