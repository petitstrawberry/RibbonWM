//! macOS integration. UI writes/observers stay on main; discovery probes are read-only.
#[cfg(not(target_os = "macos"))]
compile_error!(
    "ribbon-macos requires macOS; ribbon-core can be tested independently on other platforms"
);

pub mod backend;
pub mod demo;
pub mod input;

use anyhow::{Context, Result, bail};
use ribbon_core::{NativeSpaceId, Rect, WindowId};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, c_char, c_void};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Display {
    pub id: String,
    pub display_id: u32,
    pub name: String,
    pub frame: Rect,
    pub viewport: Rect,
    pub scale: f64,
    pub native_space: NativeSpaceId,
    pub native_fullscreen: bool,
    pub primary: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub pid: i32,
    pub app: String,
    #[serde(default)]
    pub bundle_id: String,
    pub title: String,
    pub layer: i32,
    pub onscreen: bool,
    pub bounds: Rect,
    /// Untransformed surface geometry; presentation bounds can be clipped/scaled.
    #[serde(default)]
    pub surface_bounds: Option<Rect>,
    pub native_spaces: Vec<NativeSpaceId>,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default)]
    pub sticky_known: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Application {
    pub pid: i32,
    pub app: String,
    pub bundle_id: String,
}

unsafe extern "C" {
    fn ribbon_query_json(kind: i32) -> *mut c_char;
    fn ribbon_session_active() -> i32;
    fn ribbon_query_windows_json(ids: *const u32, count: usize) -> *mut c_char;
    fn ribbon_free(pointer: *mut c_void);
    fn ribbon_ax_trusted() -> i32;
    fn ribbon_wait_for_events(seconds: f64);
    fn ribbon_permission_host_initialize();
    fn ribbon_owned_probe_accessible(pid: i32) -> i32;
    fn ribbon_ax_request_permission() -> i32;
    fn ribbon_resize_window(wid: u32, pid: i32, rect: Rect) -> i32;
    fn ribbon_resize_window_observed(
        wid: u32,
        pid: i32,
        rect: Rect,
        progress: extern "C" fn(*mut c_void) -> i32,
        context: *mut c_void,
    ) -> i32;
    fn ribbon_resize_begin(
        wid: u32,
        pid: i32,
        rect: Rect,
        calibration_ax: Rect,
        calibration_surface: Rect,
        progress: extern "C" fn(*mut c_void) -> i32,
        context: *mut c_void,
        pending: *mut *mut c_void,
    ) -> i32;
    fn ribbon_settlement_poll(context: *mut c_void) -> i32;
    fn ribbon_settlement_release(context: *mut c_void);
    fn ribbon_probe_application(pid: i32, candidates: *const u32, count: usize) -> *mut c_char;
    fn ribbon_window_geometry(wid: u32, pid: i32, rect: *mut Rect) -> i32;
    fn ribbon_window_presentation(wid: u32, pid: i32, rect: *mut Rect, surface: *mut Rect) -> i32;
    fn ribbon_restore_window(wid: u32, pid: i32, rect: Rect) -> i32;
    fn ribbon_focus_window(wid: u32, pid: i32) -> i32;
    fn ribbon_frontmost_pid() -> i32;
    fn ribbon_focused_window(pid: i32) -> u32;
    fn ribbon_window_manageable(wid: u32, pid: i32) -> i32;
    fn ribbon_window_owner(wid: u32) -> i32;
    fn ribbon_forget_window(wid: u32);
    fn ribbon_pointer(x: *mut f64, y: *mut f64);
    fn ribbon_left_mouse_down() -> i32;
    fn ribbon_mouse_state() -> MouseState;
    fn ribbon_mission_control_active() -> i32;
    fn ribbon_left_mouse_down_age() -> f64;
    fn ribbon_watch_application(pid: i32) -> i32;
    fn ribbon_unwatch_application(pid: i32);
    fn ribbon_events() -> u32;
    fn ribbon_take_closed_windows(windows: *mut ClosedWindow, capacity: usize) -> usize;
    fn ribbon_take_changed_windows(windows: *mut u32, capacity: usize) -> usize;
    fn ribbon_stop_observing();
}

fn query<T: serde::de::DeserializeOwned>(kind: i32) -> Result<T> {
    // SAFETY: native code returns a malloc-owned, NUL-terminated JSON buffer or null.
    let pointer = unsafe { ribbon_query_json(kind) };
    if pointer.is_null() {
        bail!("Native inventory returned no data");
    }
    // SAFETY: the buffer lives until ribbon_free; CStr borrows it only while parsing.
    let result = serde_json::from_slice(unsafe { CStr::from_ptr(pointer) }.to_bytes());
    // SAFETY: release exactly once with the matching native allocator.
    unsafe { ribbon_free(pointer.cast()) };
    result.context("Invalid native inventory JSON")
}
pub fn displays() -> Result<Vec<Display>> {
    query(0)
}
pub fn windows() -> Result<Vec<Window>> {
    query(1)
}
/// Cheap metadata only. No per-window Space, tag, or physical-bounds queries.
pub fn window_summaries() -> Result<Vec<Window>> {
    query(3)
}
/// Resolve only Rust-selected candidates and existing leases, including hidden ones.
pub fn window_details(ids: &[WindowId]) -> Result<Vec<Window>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<u32> = ids.iter().map(|id| id.0).collect();
    // SAFETY: native borrows this initialized ID array only during the call.
    let raw = unsafe { ribbon_query_windows_json(ids.as_ptr(), ids.len()) };
    if raw.is_null() {
        bail!("Selected window inventory unavailable");
    }
    // SAFETY: non-null native result is an owned NUL-terminated buffer.
    let result = serde_json::from_slice(unsafe { CStr::from_ptr(raw) }.to_bytes());
    // SAFETY: release the buffer once after the deserializer has copied it.
    unsafe { ribbon_free(raw.cast()) };
    result.context("Invalid selected window inventory")
}
pub fn applications() -> Result<Vec<Application>> {
    query(2)
}
pub fn session_active() -> bool {
    // SAFETY: read-only session dictionary query with no retained caller data.
    unsafe { ribbon_session_active() != 0 }
}
#[derive(Debug, Deserialize)]
pub struct WindowProbe {
    pub id: WindowId,
    pub geometry: Rect,
}
#[derive(Debug, Deserialize)]
pub struct ApplicationProbe {
    pub members: Option<Vec<WindowId>>,
    pub ready: Vec<WindowProbe>,
}
/// Caller must filter excluded applications before any AX access. This probe
/// owns all its AX references and never mutates the main-thread element cache.
pub fn probe_application(pid: i32, candidates: &[u32]) -> Result<ApplicationProbe> {
    // SAFETY: read-only input slice lives through this synchronous call; native
    // returns a separately allocated string released exactly once below.
    let raw = unsafe { ribbon_probe_application(pid, candidates.as_ptr(), candidates.len()) };
    if raw.is_null() {
        bail!("Application {pid} membership probe unavailable");
    }
    // SAFETY: a non-null probe result is a NUL-terminated owned JSON buffer.
    let result = unsafe { serde_json::from_slice(CStr::from_ptr(raw).to_bytes()) };
    // SAFETY: parsing borrowed the buffer; it is released once after parsing.
    unsafe { ribbon_free(raw.cast()) };
    Ok(result?)
}
#[derive(Default)]
pub struct EventSource(std::marker::PhantomData<std::rc::Rc<()>>);
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ClosedWindow {
    pub wid: u32,
    pub pid: i32,
    pub withdrawn: i32,
}
impl EventSource {
    pub const WINDOWS: u32 = 1;
    pub const FOCUS: u32 = 2;
    pub const GEOMETRY: u32 = 4;
    pub const APPS: u32 = 8;
    /// Call only for apps that have passed the WM exclusion policy.
    pub fn watch(&self, pid: i32) -> bool {
        // SAFETY: registers an observer for a value-only process ID on main.
        unsafe { ribbon_watch_application(pid) != 0 }
    }
    pub fn unwatch(&self, pid: i32) {
        // SAFETY: removes only a previously registered main-thread observer.
        unsafe { ribbon_unwatch_application(pid) }
    }
    pub fn drain(&self) -> u32 {
        // SAFETY: bounded main-runloop notification processing, no pointers.
        unsafe { ribbon_events() }
    }
    pub fn closed_windows(&self) -> Vec<ClosedWindow> {
        let mut windows = [ClosedWindow::default(); 256];
        // SAFETY: native writes at most capacity initialized value-only records.
        let count = unsafe { ribbon_take_closed_windows(windows.as_mut_ptr(), windows.len()) };
        windows[..count].to_vec()
    }
    pub fn changed_windows(&self) -> Vec<WindowId> {
        let mut windows = [0u32; 256];
        // SAFETY: main-thread queue drain writes at most capacity u32 IDs.
        let count = unsafe { ribbon_take_changed_windows(windows.as_mut_ptr(), windows.len()) };
        windows[..count].iter().copied().map(WindowId).collect()
    }
}
impl Drop for EventSource {
    fn drop(&mut self) {
        // SAFETY: releases the main-thread observers owned by this daemon.
        unsafe { ribbon_stop_observing() }
    }
}
pub fn accessibility_trusted() -> bool {
    // SAFETY: queries permission without displaying a prompt or accepting pointers.
    unsafe { ribbon_ax_trusted() != 0 }
}
/// Keep native permission/workspace notifications flowing while idle.
pub fn wait_for_events(seconds: f64) {
    // SAFETY: value-only, bounded main-thread run-loop wait.
    unsafe { ribbon_wait_for_events(seconds) }
}
pub fn initialize_permission_host() {
    // SAFETY: initializes this owned accessory process without any window.
    unsafe { ribbon_permission_host_initialize() }
}
/// Probe only a child spawned and retained by the calling service.
pub fn owned_probe_accessible(pid: i32) -> bool {
    // SAFETY: read-only AX request to the caller's own separate host process.
    unsafe { ribbon_owned_probe_accessible(pid) != 0 }
}
/// Ask macOS to inform the user if this process lacks Accessibility permission.
/// The prompt is asynchronous; the return value reports the current trust only.
pub fn request_accessibility_permission() -> bool {
    // SAFETY: the native wrapper owns the options dictionary and takes no pointers.
    unsafe { ribbon_ax_request_permission() != 0 }
}
pub fn pointer() -> (f64, f64) {
    let (mut x, mut y) = (0.0, 0.0);
    // SAFETY: both pointers refer to live, writable doubles.
    unsafe { ribbon_pointer(&mut x, &mut y) };
    (x, y)
}
pub fn left_mouse_down() -> bool {
    // SAFETY: read-only global button state; no app UI or input injection.
    unsafe { ribbon_left_mouse_down() != 0 }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MouseState {
    pub x: f64,
    pub y: f64,
    pub window: u32,
    pub down: u32,
    pub dragged: u32,
}
pub fn mouse_state() -> MouseState {
    // SAFETY: listen-only event tap, main-thread value-only state.
    unsafe { ribbon_mouse_state() }
}
pub fn mission_control_active() -> bool {
    // SAFETY: main-thread Dock observer and metadata query.
    unsafe { ribbon_mission_control_active() != 0 }
}
pub fn left_mouse_down_age() -> f64 {
    // SAFETY: value-only query of the combined session's last button event.
    unsafe { ribbon_left_mouse_down_age() }
}
pub fn resize_window(id: WindowId, pid: i32, logical_frame: Rect) -> Result<()> {
    if !logical_frame.valid() {
        bail!("Invalid window dimensions");
    }
    // SAFETY: native code resolves an AX window by ID; arguments contain no pointers.
    let code = unsafe { ribbon_resize_window(id.0, pid, logical_frame) };
    if code != 0 {
        bail!("Accessibility resize for window {} failed ({code})", id.0);
    }
    Ok(())
}
/// Keep the compositor lease visible while the owner accepts AX geometry.
/// Native glue reports progress synchronously; the caller owns placement policy.
fn geometry_progress(
    mut progress: impl FnMut() -> Result<()>,
    invoke: impl FnOnce(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> i32,
) -> Result<()> {
    struct Progress<'a> {
        callback: &'a mut dyn FnMut() -> Result<()>,
        error: Option<anyhow::Error>,
    }
    extern "C" fn tick(context: *mut c_void) -> i32 {
        // SAFETY: native glue invokes this only synchronously with the live
        // Progress below. A panic must never unwind through Objective-C.
        let state = unsafe { &mut *context.cast::<Progress<'_>>() };
        if state.error.is_some() {
            return 1;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut state.callback)) {
            Ok(Ok(())) => 0,
            Ok(Err(error)) => {
                state.error = Some(error);
                1
            }
            Err(_) => {
                state.error = Some(anyhow::anyhow!("Geometry progress callback panicked"));
                1
            }
        }
    }
    let mut state = Progress {
        callback: &mut progress,
        error: None,
    };
    let code = invoke(tick, (&mut state as *mut Progress<'_>).cast());
    if let Some(error) = state.error {
        return Err(error.context("Compositor update during AX resize failed"));
    }
    if code != 0 {
        bail!("Accessibility geometry update failed ({code})");
    }
    Ok(())
}
pub fn resize_window_observed(
    id: WindowId,
    pid: i32,
    frame: Rect,
    progress: impl FnMut() -> Result<()>,
) -> Result<()> {
    if !frame.valid() {
        bail!("Invalid window dimensions");
    }
    geometry_progress(progress, |tick, context| {
        // SAFETY: progress context lives through this synchronous call.
        unsafe { ribbon_resize_window_observed(id.0, pid, frame, tick, context) }
    })
}
/// Owns a main-thread settlement check; AX writes have already acknowledged.
/// Polling checks one sample and never sleeps waiting for a future frame.
pub struct PendingResize {
    raw: std::ptr::NonNull<c_void>,
    _main_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl PendingResize {
    pub fn poll(&mut self) -> Result<bool> {
        // SAFETY: this owned context is live until Drop and stays on main.
        match unsafe { ribbon_settlement_poll(self.raw.as_ptr()) } {
            0 => Ok(false),
            1 => Ok(true),
            code => bail!("Native geometry did not settle ({code})"),
        }
    }
}
impl Drop for PendingResize {
    fn drop(&mut self) {
        // SAFETY: exactly one release of the owned native settlement context.
        unsafe { ribbon_settlement_release(self.raw.as_ptr()) };
    }
}
pub struct FrameCalibration {
    pub logical: Rect,
    pub surface: Rect,
}
pub fn begin_resize_window(
    id: WindowId,
    pid: i32,
    frame: Rect,
    calibration: FrameCalibration,
    progress: impl FnMut() -> Result<()>,
) -> Result<PendingResize> {
    if !frame.valid() || !calibration.logical.valid() || !calibration.surface.valid() {
        bail!("Invalid window dimensions");
    }
    let mut raw = std::ptr::null_mut();
    geometry_progress(progress, |tick, context| {
        // SAFETY: progress and output pointers live through the call; native
        // transfers its settlement allocation only on success.
        unsafe {
            ribbon_resize_begin(
                id.0,
                pid,
                frame,
                calibration.logical,
                calibration.surface,
                tick,
                context,
                &mut raw,
            )
        }
    })?;
    Ok(PendingResize {
        raw: std::ptr::NonNull::new(raw).context("Missing settlement context")?,
        _main_thread: std::marker::PhantomData,
    })
}
pub fn window_geometry(id: WindowId, pid: i32) -> Result<Rect> {
    let mut rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: rect is writable; native code validates the owning PID first.
    let code = unsafe { ribbon_window_geometry(id.0, pid, &mut rect) };
    if code != 0 || !rect.valid() {
        bail!("Reading AX geometry for window {} failed ({code})", id.0);
    }
    Ok(rect)
}
pub fn window_presentation(id: WindowId, pid: i32) -> Result<Rect> {
    window_drag_geometry(id, pid).map(|(_, shown)| shown)
}
/// Physical and presented geometry read directly from WindowServer, without AX.
pub fn window_drag_geometry(id: WindowId, pid: i32) -> Result<(Rect, Rect)> {
    let mut rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };
    let mut surface = rect;
    // SAFETY: both pointers are writable values; native verifies owner and finite geometry.
    let code = unsafe { ribbon_window_presentation(id.0, pid, &mut rect, &mut surface) };
    if code != 0 || !rect.valid() || !surface.valid() {
        bail!("Reading presentation for window {} failed ({code})", id.0);
    }
    Ok((surface, rect))
}
pub fn restore_window(window: &Window, logical: Rect) -> Result<()> {
    if !logical.valid() {
        bail!("Invalid original geometry");
    }
    // SAFETY: value-only arguments; native code checks the current owning PID before writing.
    let code = unsafe { ribbon_restore_window(window.id.0, window.pid, logical) };
    if code != 0 {
        bail!("Restoring window {} failed ({code})", window.id.0);
    }
    Ok(())
}
pub fn focus_window(id: WindowId, pid: i32) -> Result<()> {
    // SAFETY: native code resolves and raises the AX window by ID.
    let code = unsafe { ribbon_focus_window(id.0, pid) };
    if code != 0 {
        bail!("Accessibility focus for window {} failed ({code})", id.0);
    }
    Ok(())
}
pub fn frontmost_pid() -> i32 {
    // SAFETY: reads process metadata; it neither activates nor queries app UI.
    unsafe { ribbon_frontmost_pid() }
}
pub fn focused_window(pid: i32) -> Option<WindowId> {
    // SAFETY: read-only AX query, bounded and guarded by frontmost PID.
    let id = unsafe { ribbon_focused_window(pid) };
    (id != 0).then_some(WindowId(id))
}
pub fn window_manageable(id: WindowId, pid: i32) -> bool {
    // SAFETY: read-only role and attribute checks against the expected owner.
    unsafe { ribbon_window_manageable(id.0, pid) != 0 }
}
pub fn window_owner(id: WindowId) -> i32 {
    // SAFETY: value-only read of the current WindowServer owner PID.
    unsafe { ribbon_window_owner(id.0) }
}
pub fn forget_window(id: WindowId) {
    // SAFETY: releases only an in-process cached AX reference, without OS writes.
    unsafe { ribbon_forget_window(id.0) };
}
