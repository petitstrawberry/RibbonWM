use anyhow::{Result, bail};
use ribbon_core::{Action, Direction, Engine, NativeSpaceId, Rect, Settings, WindowId};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[repr(C)]
#[derive(Clone, Copy)]
struct DemoWindow {
    wid: u32,
    bounds: Rect,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct DemoInfo {
    viewport: Rect,
    windows: [DemoWindow; 3],
}
#[repr(C)]
#[derive(Clone, Copy)]
struct DemoFrame {
    wid: u32,
    visible: u32,
    dx: f64,
    dy: f64,
    clip: Rect,
}
#[repr(C)]
struct Callbacks {
    context: *mut c_void,
    initialize: extern "C" fn(*mut c_void, *const DemoInfo) -> i32,
    update: extern "C" fn(*mut c_void, f64, *mut DemoFrame, usize) -> usize,
    event: extern "C" fn(*mut c_void, i32, f64),
}
unsafe extern "C" {
    fn ribbon_demo_run(
        callbacks: *const Callbacks,
        test: i32,
        seconds: f64,
        left: f64,
        width: f64,
    ) -> i32;
}
struct State {
    engine: Engine,
    info: Option<DemoInfo>,
    test: bool,
    hidden: bool,
    failure: Option<String>,
}

extern "C" fn initialize(context: *mut c_void, info: *const DemoInfo) -> i32 {
    // SAFETY: ribbon_demo_run calls synchronously on the main thread with live pointers.
    let state = unsafe { &mut *context.cast::<State>() };
    // SAFETY: native code initialized all fields; copy before returning to AppKit.
    let info = unsafe { *info };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        state.info = Some(info);
        state
            .engine
            .update_monitor("demo", info.viewport, NativeSpaceId(0), false)?;
        for w in info.windows {
            state
                .engine
                .add_window("demo", WindowId(w.wid), Some(w.bounds.width))?;
        }
        if state.test {
            state
                .engine
                .apply("demo", &Action::Scroll { delta: 10000.0 })?;
        } else {
            state.engine.focus_window(WindowId(info.windows[0].wid))?;
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            state.failure = Some(e.to_string());
            1
        }
        Err(_) => {
            state.failure = Some("Rust demo initialization panicked".into());
            1
        }
    }
}
extern "C" fn update(
    context: *mut c_void,
    dt: f64,
    output: *mut DemoFrame,
    capacity: usize,
) -> usize {
    // SAFETY: callbacks are serialized by AppKit; State is exclusively owned by run().
    let state = unsafe { &mut *context.cast::<State>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        state.engine.tick(dt);
        let Some(info) = state.info else {
            return 0;
        };
        let plans = state.engine.placements();
        if plans.len() > capacity {
            return 0;
        }
        // SAFETY: native caller supplies capacity initialized storage slots for DemoFrame.
        let output = unsafe { std::slice::from_raw_parts_mut(output, capacity) };
        for (p, destination) in plans.iter().zip(output) {
            let Some(native) = info.windows.iter().find(|w| w.wid == p.window.0) else {
                return 0;
            };
            *destination = DemoFrame {
                wid: p.window.0,
                visible: u32::from(!state.hidden && p.clip.is_some()),
                dx: p.frame.x - native.bounds.x,
                dy: p.frame.y - native.bounds.y,
                clip: p.clip.unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                }),
            };
        }
        plans.len()
    }));
    match result {
        Ok(count) => count,
        Err(_) => {
            state.failure = Some("Rust demo frame callback panicked".into());
            0
        }
    }
}
extern "C" fn event(context: *mut c_void, kind: i32, value: f64) {
    // SAFETY: same single-threaded callback contract as initialize/update.
    let state = unsafe { &mut *context.cast::<State>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let direction = if value > 0.0 {
            Direction::Next
        } else {
            Direction::Previous
        };
        let action = match kind {
            1 => Action::FocusColumn { direction },
            2 if state.test => {
                state.hidden = true;
                return;
            }
            4 => Action::Scroll { delta: value },
            5 => {
                if let Some(w) = state
                    .info
                    .and_then(|i| i.windows.get(value as usize).copied())
                {
                    let _ = state.engine.focus_window(WindowId(w.wid));
                }
                return;
            }
            _ => return,
        };
        if let Err(e) = state.engine.apply("demo", &action) {
            eprintln!("{e}");
        }
    }));
    if result.is_err() {
        state.failure = Some("Rust demo event callback panicked".into());
    }
}

/// Opens only this process's native test surfaces. Does not require Dock injection.
pub fn run(test: bool, seconds: f64, left: f64, width: f64) -> Result<()> {
    if !seconds.is_finite() || seconds < 0.0 {
        bail!("seconds must be finite and nonnegative");
    }
    if test && seconds != 0.0 {
        bail!("Self-test uses its own fixed duration; omit --seconds");
    }
    if !left.is_finite() || left < 0.0 || !width.is_finite() || width < 548.0 {
        bail!(
            "Demo left must be finite and nonnegative; width must be finite and at least 548 points"
        );
    }
    let settings = Settings {
        column_width: 400.0,
        gap: 20.0,
        ..Settings::default()
    };
    let mut state = State {
        engine: Engine::new(settings)?,
        info: None,
        test,
        hidden: false,
        failure: None,
    };
    let callbacks = Callbacks {
        context: (&mut state as *mut State).cast(),
        initialize,
        update,
        event,
    };
    // SAFETY: callbacks/state stay alive until the synchronous AppKit loop returns.
    // Entry is invoked on the process main thread by the CLI.
    let result = unsafe { ribbon_demo_run(&callbacks, i32::from(test), seconds, left, width) };
    if let Some(e) = state.failure {
        bail!("{e}");
    }
    if result != 0 {
        bail!("Native demo failed; inspect the printed checks and event posting permission");
    }
    Ok(())
}
