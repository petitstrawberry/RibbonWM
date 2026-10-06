//! Main-thread gesture routing. No AX, layout or IPC work runs in the tap.
use anyhow::{Result, bail};
use ribbon_core::{NativeSpaceId, Rect, ScrollModifier, Settings};
use serde::Serialize;
use std::ffi::{CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

const END: u32 = 8 | 16;
const MODIFIERS: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
const TAIL_TIMEOUT: f64 = 1.2;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Touch {
    identity: u64,
    x: f64,
    y: f64,
    phase: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Sample {
    dx: f64,
    dy: f64,
    x: f64,
    y: f64,
    time: f64,
    flags: u64,
    phase: u32,
    momentum: u32,
    precise: u32,
    kind: u32,
    count: u32,
    touches: [Touch; 8],
}
unsafe extern "C" {
    fn ribbon_start_scroll(
        callback: extern "C" fn(*mut c_void, *const Sample) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn ribbon_stop_scroll();
    fn ribbon_scroll_alive() -> i32;
    fn ribbon_input_time() -> f64;
    fn ribbon_input_trusted() -> i32;
    fn ribbon_request_input_permission() -> i32;
    fn ribbon_probe_show(text: *const c_char);
    fn ribbon_probe_action() -> u32;
    fn ribbon_probe_close();
}
pub fn trusted() -> bool {
    // SAFETY: read-only permission check with no arguments.
    unsafe { ribbon_input_trusted() != 0 }
}
pub fn request_permission() -> bool {
    // SAFETY: asks the OS; never grants permission itself.
    unsafe { ribbon_request_input_permission() != 0 }
}
pub struct ProbeWindow;
impl ProbeWindow {
    pub fn show(&self, text: &str) -> Result<()> {
        let text = CString::new(text)?;
        // SAFETY: native synchronously copies text; UI is owned by this process.
        unsafe { ribbon_probe_show(text.as_ptr()) };
        Ok(())
    }
    pub fn action(&self) -> u32 {
        // SAFETY: bounded dispatch of this process's own AppKit events on main.
        unsafe { ribbon_probe_action() }
    }
}
impl Drop for ProbeWindow {
    fn drop(&mut self) {
        // SAFETY: closes only this process's owned diagnostic window.
        unsafe { ribbon_probe_close() };
    }
}
/// Visible, explicitly started diagnostic. Classification is simulated; every
/// event still passes to macOS and no layout/window operation is performed.
pub fn monitor(seconds: f64, fingers: u32) -> Result<()> {
    if !seconds.is_finite() || !(1.0..=120.0).contains(&seconds) || !(2..=4).contains(&fingers) {
        bail!("Use 1..120 seconds and 2..4 fingers");
    }
    use std::io::Write;
    use std::time::{Duration, Instant};
    let window = ProbeWindow;
    window.show(&format!("{fingers}本指モードの計測\n\n「計測開始」を押してから左右にスワイプ。\n入力はすべてmacOSへ通します。\nウィンドウ配置は変更しません。\nSpacesの通常動作はそのまま起きます。"))?;
    let events = crate::EventSource::default();
    let mut source = None;
    let mut began = None;
    let mut last_draw = Instant::now();
    let waiting = Instant::now();
    let mut contacts = 0;
    let mut peak = 0;
    let mut packets = 0;
    let mut momentum = 0;
    let mut routed = 0;
    let mut distance = 0.0;
    loop {
        let action = window.action();
        if action & 2 != 0 || (began.is_none() && waiting.elapsed() > Duration::from_secs(600)) {
            break;
        }
        if action & 1 != 0 && began.is_none() {
            source = Some(InputSource::probe(fingers)?);
            began = Some(Instant::now());
        }
        events.drain();
        if let Some(source) = &mut source {
            for p in source.drain_trace() {
                if p.kind == "touch" {
                    contacts = p.contacts;
                    peak = peak.max(contacts);
                }
                if p.kind == "scroll" {
                    packets += 1;
                    if p.momentum != 0 {
                        momentum += 1;
                    }
                }
                if p.would_consume {
                    routed += 1;
                    distance += p.routed_delta;
                }
                println!("{}", serde_json::to_string(&p)?);
            }
            std::io::stdout().flush()?;
        }
        if let Some(began) = began {
            let remaining = (seconds - began.elapsed().as_secs_f64()).max(0.0);
            if last_draw.elapsed() >= Duration::from_millis(100) {
                window.show(&format!("{fingers}本指モード — 計測{} / 残り {remaining:.1}秒\n\n接触数: {contacts} (最大 {peak})\nスクロール: {packets} / 慣性: {momentum}\n横スクロール判定: {routed} / 距離: {distance:.1} pt\n\n入力を止めず、WM配置も変えません。\n終わったらこのウィンドウを閉じてください。",
                    if remaining==0.0 { "終了" } else { "中" }))?;
                last_draw = Instant::now();
            }
            if remaining == 0.0 {
                source = None;
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    Ok(())
}
#[derive(Clone, PartialEq)]
pub struct Target {
    pub monitor: String,
    pub space: NativeSpaceId,
    pub viewport: Rect,
}
pub struct ScrollDelta {
    pub monitor: String,
    pub space: NativeSpaceId,
    pub delta: f64,
    viewport: Rect,
}
#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Pending,
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, PartialEq)]
enum Driver {
    Wheel,
    Touch,
}
struct Capture {
    target: Target,
    time: f64,
    fingers_down: bool,
    axis: Axis,
    driver: Driver,
    origin: Vec<Touch>,
    previous: Vec<Touch>,
    dx: f64,
    dy: f64,
}
#[derive(Clone, PartialEq)]
struct Config {
    modifier: Option<ScrollModifier>,
    fingers: u32,
    sensitivity: f64,
    reverse: bool,
    momentum: bool,
}
impl From<&Settings> for Config {
    fn from(s: &Settings) -> Self {
        Self {
            modifier: s.gesture_modifier,
            fingers: s.gesture_fingers,
            sensitivity: s.gesture_sensitivity,
            reverse: s.gesture_reverse,
            momentum: s.gesture_momentum,
        }
    }
}
#[derive(Serialize)]
pub struct Trace {
    time: f64,
    kind: &'static str,
    contacts: u32,
    dx: f64,
    dy: f64,
    phase: u32,
    momentum: u32,
    precise: bool,
    consumed: bool,
    would_consume: bool,
    routed_delta: f64,
}
struct State {
    config: Config,
    targets: Vec<Target>,
    capture: Option<Capture>,
    pending: Vec<ScrollDelta>,
    probe: bool,
    trace: Vec<Trace>,
}
impl State {
    fn begin(&mut self, s: Sample, driver: Driver, contacts: Vec<Touch>) -> bool {
        if s.flags & MODIFIERS != self.config.modifier.map_or(0, ScrollModifier::event_mask) {
            return false;
        }
        let Some(target) = self
            .targets
            .iter()
            .find(|t| t.viewport.contains(s.x, s.y))
            .cloned()
        else {
            return false;
        };
        self.capture = Some(Capture {
            target,
            time: s.time,
            fingers_down: true,
            axis: Axis::Pending,
            driver,
            origin: contacts.clone(),
            previous: contacts,
            dx: 0.0,
            dy: 0.0,
        });
        true
    }
    fn expire(&mut self, time: f64) {
        if self
            .capture
            .as_ref()
            .is_some_and(|c| time < c.time || time - c.time >= TAIL_TIMEOUT)
        {
            self.capture = None;
        }
    }
    fn handle(&mut self, s: Sample) -> bool {
        if [s.dx, s.dy, s.x, s.y, s.time]
            .iter()
            .any(|v| !v.is_finite())
            || s.count > 8
        {
            return false;
        }
        self.expire(s.time);
        if s.kind == 1 {
            self.touch(s)
        } else if s.kind == 0 {
            self.wheel(s)
        } else {
            false
        }
    }
    fn touch(&mut self, s: Sample) -> bool {
        if self.config.fingers == 2 {
            return false;
        } // AppKit's native scroll stream owns this mode.
        let contacts: Vec<_> = s.touches[..s.count as usize]
            .iter()
            .copied()
            .filter(|t| t.phase & END == 0)
            .collect();
        if contacts.iter().any(|t| {
            !t.x.is_finite()
                || !t.y.is_finite()
                || !(0.0..=1.0).contains(&t.x)
                || !(0.0..=1.0).contains(&t.y)
        }) {
            return false;
        }
        // Ended contacts are excluded from the active count. Keep only a
        // captured horizontal tail for macOS momentum after the last lift.
        if s.phase & END != 0 || contacts.is_empty() {
            let captured = self
                .capture
                .as_ref()
                .is_some_and(|c| c.axis == Axis::Horizontal);
            if let Some(c) = &mut self.capture {
                c.fingers_down = false;
                c.time = s.time;
            }
            if !captured || s.phase & 16 != 0 {
                self.capture = None;
            }
            return captured;
        }
        if contacts.len() != self.config.fingers as usize {
            // Fingers rarely lift in the exact same packet. Retain a captured
            // tail through staggered lifts, without moving from a partial set.
            if contacts.len() < self.config.fingers as usize
                && self
                    .capture
                    .as_ref()
                    .is_some_and(|c| c.axis == Axis::Horizontal && c.driver == Driver::Touch)
            {
                let c = self.capture.as_mut().expect("captured gesture");
                c.fingers_down = false;
                c.time = s.time;
                return true;
            }
            self.capture = None;
            return false;
        }
        if s.phase & 1 != 0 {
            self.capture = None;
        }
        if self.capture.is_none() {
            self.begin(s, Driver::Touch, contacts);
            return false;
        }
        let c = self.capture.as_mut().expect("capture selected");
        if !c.fingers_down || c.driver != Driver::Touch {
            self.capture = None;
            self.begin(s, Driver::Touch, contacts);
            return false;
        }
        // NSTouch sets have no ordering. Match persistent identity, never index.
        let movement = |before: &[Touch]| -> Option<(f64, f64)> {
            let mut dx = 0.0;
            let mut dy = 0.0;
            for t in &contacts {
                let old = before.iter().find(|p| p.identity == t.identity)?;
                dx += t.x - old.x;
                dy += t.y - old.y;
            }
            Some((dx / contacts.len() as f64, dy / contacts.len() as f64))
        };
        let Some(total) = movement(&c.origin) else {
            self.capture = None;
            return false;
        };
        let Some(step) = movement(&c.previous) else {
            self.capture = None;
            return false;
        };
        c.time = s.time;
        c.previous = contacts;
        c.axis = axis(c.axis, total.0, total.1, 0.003);
        if c.axis != Axis::Horizontal {
            return false;
        }
        // Normalized contacts drive the strip directly while fingers are down.
        // Use the total on acquisition so the intent threshold loses no distance.
        let raw = if c.dx == 0.0 && c.dy == 0.0 {
            total.0
        } else {
            step.0
        };
        c.dx = total.0;
        c.dy = total.1;
        let delta = -raw * c.target.viewport.width;
        self.enqueue(delta);
        true
    }
    fn wheel(&mut self, s: Sample) -> bool {
        if s.precise == 0 || (s.phase == 0 && s.momentum == 0) {
            return false;
        }
        if s.momentum != 0 {
            let Some(c) = &mut self.capture else {
                return false;
            };
            if c.axis != Axis::Horizontal {
                return false;
            }
            c.fingers_down = false;
            c.time = s.time;
            if self.config.momentum {
                self.enqueue(-s.dx);
            }
            if s.momentum & END != 0 {
                self.capture = None;
            }
            return true;
        }
        if self.config.fingers != 2 {
            if s.phase & 1 != 0 && self.capture.as_ref().is_some_and(|c| !c.fingers_down) {
                self.capture = None;
            }
            // A raw contact packet and a native wheel packet may describe the
            // same motion. Only contacts drive this mode; consume the duplicate.
            return self
                .capture
                .as_ref()
                .is_some_and(|c| c.axis == Axis::Horizontal && c.driver == Driver::Touch);
        }
        if s.phase & 1 != 0 {
            self.capture = None;
        }
        if self.capture.is_none() && !self.begin(s, Driver::Wheel, Vec::new()) {
            return false;
        }
        let c = self.capture.as_mut().expect("capture selected");
        c.time = s.time;
        c.dx += s.dx;
        c.dy += s.dy;
        let old_axis = c.axis;
        c.axis = axis(c.axis, c.dx, c.dy, 3.0);
        let captured = c.axis == Axis::Horizontal;
        let delta = if old_axis == Axis::Pending {
            -c.dx
        } else {
            -s.dx
        };
        if s.phase & END != 0 {
            c.fingers_down = false;
        }
        if captured {
            self.enqueue(delta);
        }
        if s.phase & 16 != 0 || (s.phase & 8 != 0 && !captured) {
            self.capture = None;
        }
        captured
    }
    fn enqueue(&mut self, raw: f64) {
        let c = self.capture.as_ref().expect("capture selected");
        let direction = if self.config.reverse { -1.0 } else { 1.0 };
        let delta = (raw * self.config.sensitivity * direction).clamp(-10000.0, 10000.0);
        if delta == 0.0 {
            return;
        }
        if let Some(p) = self
            .pending
            .iter_mut()
            .find(|p| p.monitor == c.target.monitor && p.space == c.target.space)
        {
            p.delta = (p.delta + delta).clamp(-10000.0, 10000.0);
        } else {
            self.pending.push(ScrollDelta {
                monitor: c.target.monitor.clone(),
                space: c.target.space,
                delta,
                viewport: c.target.viewport,
            });
        }
    }
}
fn axis(old: Axis, dx: f64, dy: f64, threshold: f64) -> Axis {
    if old != Axis::Pending || dx.abs().max(dy.abs()) < threshold {
        return old;
    }
    if dx.abs() > dy.abs() * 1.25 {
        Axis::Horizontal
    } else if dy.abs() > dx.abs() * 1.25 {
        Axis::Vertical
    } else {
        Axis::Pending
    }
}
extern "C" fn callback(context: *mut c_void, sample: *const Sample) -> i32 {
    // SAFETY: native invokes synchronously on main with a stable Box, only
    // until Drop invalidates/removes the run-loop source.
    catch_unwind(AssertUnwindSafe(|| unsafe {
        let state = &mut *context.cast::<State>();
        let s = *sample;
        let would_consume = state.handle(s);
        let consumed = !state.probe && would_consume;
        if state.probe && state.trace.len() < 1024 {
            state.trace.push(Trace {
                time: s.time,
                kind: if s.kind == 1 { "touch" } else { "scroll" },
                contacts: s.count,
                dx: s.dx,
                dy: s.dy,
                phase: s.phase,
                momentum: s.momentum,
                precise: s.precise != 0,
                consumed,
                would_consume,
                routed_delta: state.pending.iter().map(|p| p.delta).sum(),
            });
        }
        if state.probe {
            state.pending.clear();
        }
        consumed
    }))
    .unwrap_or(false) as i32
}
pub struct InputSource {
    state: Box<State>,
    installed: bool,
    _main_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl InputSource {
    pub fn start(settings: &Settings) -> Result<Self> {
        Self::create(settings, false)
    }
    /// Observe transport only; diagnostic packets always pass through unchanged.
    pub fn probe(fingers: u32) -> Result<Self> {
        let settings = Settings {
            gesture_fingers: fingers,
            ..Settings::default()
        };
        settings.validate()?;
        let mut source = Self::create(&settings, true)?;
        source.state.targets = crate::displays()?
            .into_iter()
            .filter(|d| !d.native_fullscreen)
            .map(|d| Target {
                monitor: d.id,
                space: d.native_space,
                viewport: d.viewport,
            })
            .collect();
        Ok(source)
    }
    fn create(settings: &Settings, probe: bool) -> Result<Self> {
        let mut source = Self {
            state: Box::new(State {
                config: Config::from(settings),
                targets: Vec::new(),
                capture: None,
                pending: Vec::new(),
                probe,
                trace: Vec::new(),
            }),
            installed: false,
            _main_thread: std::marker::PhantomData,
        };
        source.install()?;
        Ok(source)
    }
    fn install(&mut self) -> Result<()> {
        // SAFETY: Box has a stable address; source remains main-thread-only.
        if unsafe { ribbon_start_scroll(callback, (&mut *self.state as *mut State).cast()) } == 0 {
            bail!(
                "Gesture event tap unavailable; grant Accessibility/Input Monitoring to RibbonWM"
            );
        }
        self.installed = true;
        Ok(())
    }
    pub fn update(&mut self, settings: &Settings, targets: Vec<Target>) -> Result<()> {
        self.state.configure(Config::from(settings), targets);
        // SAFETY: same monotonic clock as CGEvent timestamps; no pointers.
        self.state.expire(unsafe { ribbon_input_time() });
        // SAFETY: main-thread tap health check, repairing disable after sleep.
        if unsafe { ribbon_scroll_alive() } == 0 {
            self.state.capture = None;
            self.state.pending.clear();
            // SAFETY: remove this source's callback before reinstalling it.
            unsafe { ribbon_stop_scroll() };
            self.installed = false;
            self.install()?;
        }
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.state.capture = None;
        self.state.pending.clear();
    }
    pub fn drain(&mut self) -> Vec<ScrollDelta> {
        std::mem::take(&mut self.state.pending)
    }
    pub fn drain_trace(&mut self) -> Vec<Trace> {
        std::mem::take(&mut self.state.trace)
    }
    pub fn active(&self) -> bool {
        self.state
            .capture
            .as_ref()
            .is_some_and(|c| c.axis == Axis::Horizontal)
    }
}
impl State {
    fn configure(&mut self, config: Config, targets: Vec<Target>) {
        if self.config != config {
            self.capture = None;
            self.pending.clear();
        }
        self.config = config;
        self.capture = self.capture.take().filter(|c| targets.contains(&c.target));
        self.pending.retain(|p| {
            targets
                .iter()
                .any(|t| p.monitor == t.monitor && p.space == t.space && p.viewport == t.viewport)
        });
        self.targets = targets;
    }
}
impl Drop for InputSource {
    fn drop(&mut self) {
        // SAFETY: remove native callback before releasing its boxed context.
        if self.installed {
            // SAFETY: only the successfully installed owner can remove the tap.
            unsafe { ribbon_stop_scroll() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(fingers: u32) -> State {
        let settings = Settings {
            gesture_fingers: fingers,
            ..Settings::default()
        };
        State {
            config: Config::from(&settings),
            capture: None,
            pending: Vec::new(),
            probe: false,
            trace: Vec::new(),
            targets: vec![
                Target {
                    monitor: "left".into(),
                    space: NativeSpaceId(1),
                    viewport: Rect {
                        x: 0.0,
                        y: 30.0,
                        width: 1000.0,
                        height: 700.0,
                    },
                },
                Target {
                    monitor: "right".into(),
                    space: NativeSpaceId(2),
                    viewport: Rect {
                        x: 1000.0,
                        y: 30.0,
                        width: 1000.0,
                        height: 700.0,
                    },
                },
            ],
        }
    }
    fn wheel() -> Sample {
        Sample {
            dx: -3.375,
            x: 100.0,
            y: 100.0,
            time: 1.0,
            phase: 1,
            precise: 1,
            ..Sample::default()
        }
    }
    fn touch(count: usize, phase: u32, dx: f64, dy: f64, time: f64) -> Sample {
        let mut s = Sample {
            kind: 1,
            count: count as u32,
            phase,
            time,
            x: 100.0,
            y: 100.0,
            ..Sample::default()
        };
        for (i, t) in s.touches[..count].iter_mut().enumerate() {
            *t = Touch {
                identity: (i + 1) as u64,
                x: 0.2 + i as f64 * 0.1 + dx,
                y: 0.4 + dy,
                phase: 2,
            };
        }
        s
    }
    #[test]
    fn native_momentum_retains_fractional_movement_and_original_monitor() {
        let mut s = state(2);
        assert!(s.handle(wheel()));
        assert!(s.handle(Sample {
            x: 1400.0,
            time: 1.1,
            phase: 8,
            dx: 0.0,
            ..wheel()
        }));
        assert!(s.handle(Sample {
            x: 1400.0,
            time: 1.2,
            phase: 0,
            momentum: 1,
            dx: -0.375,
            ..wheel()
        }));
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0].monitor, "left");
        assert_eq!(s.pending[0].delta, 3.75);
        assert!(s.handle(Sample {
            time: 1.3,
            phase: 0,
            momentum: 8,
            dx: 0.0,
            ..wheel()
        }));
        assert!(s.capture.is_none());
    }
    #[test]
    fn vertical_wheels_discrete_wheels_wrong_modifiers_and_invalid_packets_pass() {
        for event in [
            Sample {
                dx: 0.0,
                dy: 10.0,
                ..wheel()
            },
            Sample {
                flags: 1 << 19,
                ..wheel()
            },
            Sample {
                precise: 0,
                ..wheel()
            },
            Sample { y: 10.0, ..wheel() },
            Sample {
                dx: f64::NAN,
                ..wheel()
            },
        ] {
            let mut s = state(2);
            assert!(!s.handle(event));
            assert!(s.pending.is_empty());
        }
        let mut s = state(2);
        assert!(!s.handle(Sample {
            dx: 0.0,
            dy: 10.0,
            ..wheel()
        }));
        assert!(!s.handle(Sample {
            phase: 4,
            time: 1.1,
            dx: 20.0,
            ..wheel()
        }));
    }
    #[test]
    fn optional_modifier_is_checked_only_at_acquisition() {
        let mut s = state(2);
        s.config.modifier = Some(ScrollModifier::Alt);
        assert!(!s.handle(wheel()));
        assert!(!s.handle(Sample {
            flags: (1 << 19) | (1 << 20),
            ..wheel()
        }));
        assert!(s.handle(Sample {
            flags: 1 << 19,
            ..wheel()
        }));
        assert!(s.handle(Sample {
            flags: 0,
            phase: 4,
            time: 1.1,
            ..wheel()
        }));
    }
    #[test]
    fn touch_identity_reordering_does_not_jump_and_duplicate_wheel_is_not_applied() {
        let mut s = state(3);
        assert!(!s.handle(touch(3, 1, 0.0, 0.0, 1.0)));
        let mut moved = touch(3, 4, -0.01, 0.0, 1.1);
        moved.touches.swap(0, 2);
        assert!(s.handle(moved));
        assert!((s.pending[0].delta - 10.0).abs() < 1e-9);
        assert!(s.handle(Sample {
            phase: 4,
            time: 1.11,
            dx: -10.0,
            ..wheel()
        }));
        assert!((s.pending[0].delta - 10.0).abs() < 1e-9);
        assert!(s.handle(touch(0, 8, 0.0, 0.0, 1.2)));
        assert!(s.handle(Sample {
            phase: 0,
            momentum: 1,
            time: 1.3,
            dx: -0.5,
            ..wheel()
        }));
        assert!((s.pending[0].delta - 10.5).abs() < 1e-9);
    }
    #[test]
    fn wrong_finger_count_vertical_contacts_and_changed_identity_pass() {
        let mut s = state(3);
        assert!(!s.handle(touch(4, 1, 0.0, 0.0, 1.0)));
        assert!(!s.handle(touch(3, 1, 0.0, 0.0, 1.1)));
        assert!(!s.handle(touch(3, 4, 0.0, 0.01, 1.2)));
        assert!(!s.handle(touch(3, 4, 0.05, 0.02, 1.3)));
        assert!(s.pending.is_empty());
        let mut s = state(4);
        assert!(!s.handle(touch(4, 1, 0.0, 0.0, 1.0)));
        let mut changed = touch(4, 4, -0.01, 0.0, 1.1);
        changed.touches[0].identity = 9;
        assert!(!s.handle(changed));
        assert!(s.capture.is_none());
    }
    #[test]
    fn disabled_momentum_is_consumed_without_leaking_into_apps() {
        let mut s = state(2);
        s.config.momentum = false;
        s.config.reverse = true;
        s.config.sensitivity = 2.0;
        assert!(s.handle(wheel()));
        assert_eq!(s.pending[0].delta, -6.75);
        assert!(s.handle(Sample {
            phase: 0,
            momentum: 1,
            time: 1.1,
            ..wheel()
        }));
        assert_eq!(s.pending[0].delta, -6.75);
        assert!(!s.handle(Sample {
            phase: 0,
            momentum: 1,
            time: 3.0,
            ..wheel()
        }));
        assert!(s.capture.is_none());
    }
    #[test]
    fn c_packet_layout_includes_all_touch_slots() {
        assert_eq!(std::mem::size_of::<Touch>(), 32);
        assert_eq!(std::mem::size_of::<Sample>(), 328);
        assert_eq!(std::mem::offset_of!(Sample, touches), 72);
    }
    #[test]
    fn staggered_finger_lift_retains_native_tail_but_not_a_new_scroll() {
        let mut s = state(3);
        s.handle(touch(3, 1, 0.0, 0.0, 1.0));
        assert!(s.handle(touch(3, 4, -0.01, 0.0, 1.1)));
        assert!(s.handle(touch(2, 4, -0.02, 0.0, 1.2)));
        assert!((s.pending[0].delta - 10.0).abs() < 1e-9);
        assert!(s.handle(touch(0, 8, 0.0, 0.0, 1.3)));
        assert!(!s.handle(Sample {
            time: 1.4,
            ..wheel()
        }));
        assert!(s.capture.is_none());
    }
    #[test]
    fn space_geometry_and_configuration_changes_cancel_capture_and_queued_motion() {
        for change in 0..4 {
            let mut s = state(2);
            assert!(s.handle(wheel()));
            let mut targets = s.targets.clone();
            let mut config = s.config.clone();
            match change {
                0 => targets[0].space = NativeSpaceId(9),
                1 => targets[0].viewport.y = 40.0,
                2 => config.reverse = true,
                _ => targets.clear(),
            }
            s.configure(config, targets);
            assert!(s.capture.is_none());
            assert!(s.pending.is_empty());
            assert!(!s.handle(Sample {
                phase: 0,
                momentum: 1,
                time: 1.1,
                ..wheel()
            }));
        }
    }
}
