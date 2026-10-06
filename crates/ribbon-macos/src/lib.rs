//! macOS integration. All UI entry points run on the calling main thread.
#[cfg(not(target_os = "macos"))]
compile_error!(
    "ribbon-macos requires macOS; ribbon-core can be tested independently on other platforms"
);

pub mod backend;
pub mod demo;

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
    pub native_spaces: Vec<NativeSpaceId>,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default)]
    pub sticky_known: bool,
}

unsafe extern "C" {
    fn ribbon_query_json(kind: i32) -> *mut c_char;
    fn ribbon_free(pointer: *mut c_void);
    fn ribbon_ax_trusted() -> i32;
    fn ribbon_ax_request_permission() -> i32;
    fn ribbon_resize_window(wid: u32, pid: i32, rect: Rect) -> i32;
    fn ribbon_restore_window(wid: u32, pid: i32, rect: Rect) -> i32;
    fn ribbon_focus_window(wid: u32, pid: i32) -> i32;
    fn ribbon_frontmost_pid() -> i32;
    fn ribbon_focused_window(pid: i32) -> u32;
    fn ribbon_window_manageable(wid: u32, pid: i32) -> i32;
    fn ribbon_window_owner(wid: u32) -> i32;
    fn ribbon_forget_window(wid: u32);
    fn ribbon_pointer(x: *mut f64, y: *mut f64);
    fn ribbon_left_mouse_down() -> i32;
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
pub fn accessibility_trusted() -> bool {
    // SAFETY: queries permission without displaying a prompt or accepting pointers.
    unsafe { ribbon_ax_trusted() != 0 }
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
pub fn restore_window(window: &Window) -> Result<()> {
    if !window.bounds.valid() {
        bail!("Invalid original geometry");
    }
    // SAFETY: value-only arguments; native code checks the current owning PID before writing.
    let code = unsafe { ribbon_restore_window(window.id.0, window.pid, window.bounds) };
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
