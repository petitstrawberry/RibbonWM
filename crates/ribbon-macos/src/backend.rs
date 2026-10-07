//! Bounded same-user Unix-socket client for the optional arm64e Dock payload.
use anyhow::{Context, Result, bail};
use ribbon_core::{Placement, Rect, WindowId};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 65536;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendStatus {
    pub version: u32,
    pub pid: i32,
    pub uid: u32,
    pub controlled: usize,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct StickyWindow {
    pub wid: u32,
    pub pid: i32,
    pub enabled: bool,
}
enum NativePosition<'a> {
    Settled(&'a BTreeSet<WindowId>),
    Resize(WindowId, Rect),
}
#[derive(Serialize)]
struct Update {
    wid: u32,
    pid: i32,
    frame: Rect,
    clip: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewport: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drag_frame: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_viewport: Option<Rect>,
    anchor: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_frame: Option<Rect>,
}
#[derive(Serialize)]
struct Request<'a> {
    op: &'a str,
    session: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    updates: Option<Vec<Update>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stickies: Option<&'a [StickyWindow]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<&'a StickyWindow>,
}

pub fn uid() -> u32 {
    // SAFETY: getuid has no arguments, returns the process real UID.
    unsafe { libc::getuid() }
}
pub fn socket_path() -> PathBuf {
    PathBuf::from(format!("/tmp/ribbonwm-{}/backend.sock", uid()))
}

/// Packaged services must use the payload shipped with their executable.
/// Development binaries have no installed lib directory and use the explicit loader.
pub fn expected_build() -> Result<Option<String>> {
    let exe = std::env::current_exe()?;
    let Some(root) = exe.parent().and_then(|p| p.parent()) else {
        return Ok(None);
    };
    let payload = root.join("lib/ribbonwm/ribbon-payload.dylib");
    if !payload.exists() {
        return Ok(None);
    }
    Ok(std::fs::canonicalize(payload)?
        .file_name()
        .map(|s| s.to_string_lossy().into_owned()))
}

pub struct Backend {
    session: String,
    armed: Cell<bool>,
    version: Cell<u32>,
    sticky: Cell<bool>,
    interactive: Cell<bool>,
    lifecycle: Cell<bool>,
    topmost: Cell<bool>,
}
impl Backend {
    pub fn connect() -> Result<Self> {
        let b = Self {
            armed: Cell::new(false),
            version: Cell::new(0),
            sticky: Cell::new(false),
            interactive: Cell::new(false),
            lifecycle: Cell::new(false),
            topmost: Cell::new(false),
            session: format!(
                "{}-{}-{}",
                uid(),
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ),
        };
        let status = b.status()?;
        b.lifecycle.set(
            status.capabilities.iter().any(|c| c == "overview")
                && status.capabilities.iter().any(|c| c == "finish")
                && status.capabilities.iter().any(|c| c == "pointer_drag")
                && status.capabilities.iter().any(|c| c == "window_groups")
                && status.capabilities.iter().any(|c| c == "native_anchor")
                && status.capabilities.iter().any(|c| c == "resize_anchor"),
        );
        if !matches!(status.version, 1 | 2) || status.uid != uid() {
            bail!("Unexpected payload version or user");
        }
        b.version.set(status.version);
        b.topmost
            .set(status.capabilities.iter().any(|c| c == "topmost"));
        b.sticky
            .set(status.capabilities.iter().any(|s| s == "sticky"));
        b.interactive
            .set(status.capabilities.iter().any(|s| s == "interactive_clip"));
        Ok(b)
    }
    fn request(
        &self,
        op: &str,
        updates: Option<Vec<Update>>,
        stickies: Option<&[StickyWindow]>,
        window: Option<&StickyWindow>,
    ) -> Result<serde_json::Value> {
        let mut stream = UnixStream::connect(socket_path())
            .context("Dock payload unavailable; run doctor and the explicit native load script")?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut body = serde_json::to_vec(&Request {
            op,
            session: &self.session,
            updates,
            stickies,
            window,
        })?;
        if body.len() as u64 >= LIMIT {
            bail!("Frame exceeds protocol limit");
        }
        body.push(b'\n');
        stream.write_all(&body)?;
        let mut reply = Vec::new();
        BufReader::new(stream)
            .take(LIMIT)
            .read_until(b'\n', &mut reply)?;
        if reply.last() != Some(&b'\n') || reply.len() as u64 >= LIMIT {
            bail!("Truncated or oversized payload response");
        }
        let value: serde_json::Value = serde_json::from_slice(&reply)?;
        if value["ok"] != true {
            bail!(
                "Payload: {}",
                value["error"].as_str().unwrap_or("request rejected")
            );
        }
        Ok(value)
    }
    pub fn status(&self) -> Result<BackendStatus> {
        Ok(serde_json::from_value(
            self.request("hello", None, None, None)?,
        )?)
    }
    pub fn require_live_version(&self) -> Result<()> {
        if self.version.get() != 2 || !self.interactive.get() || !self.lifecycle.get() {
            bail!(
                "Reload the Dock backend with nix develop -c sh scripts/load-backend.sh (protocol 2, interactive_clip, overview, finish, pointer_drag, window_groups, native_anchor and resize_anchor required)"
            );
        }
        Ok(())
    }
    pub fn frame(&self, placements: &[Placement], owners: &BTreeMap<WindowId, i32>) -> Result<()> {
        self.frame_with_sticky(placements, owners, &[])
    }
    pub fn require_topmost(&self) -> Result<()> {
        if !self.topmost.get() {
            bail!("Reload the Dock backend: topmost capability is required");
        }
        Ok(())
    }
    pub fn set_topmost(&self, window: &StickyWindow) -> Result<()> {
        self.require_topmost()?;
        self.armed.set(true);
        self.request("topmost", None, None, Some(window))?;
        Ok(())
    }
    pub fn set_sticky(&self, window: &StickyWindow) -> Result<()> {
        if !self.sticky.get() {
            bail!("Reload the Dock backend: sticky capability is required");
        }
        self.armed.set(true);
        self.request("sticky", None, None, Some(window))?;
        Ok(())
    }
    pub fn frame_with_sticky(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
    ) -> Result<()> {
        self.send_frame(placements, owners, stickies, None, None, None)
    }
    pub fn frame_with_sticky_in_viewports(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
        viewports: &BTreeMap<String, Rect>,
    ) -> Result<()> {
        self.send_frame(placements, owners, stickies, None, Some(viewports), None)
    }
    pub fn settled_frame(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
        viewports: &BTreeMap<String, Rect>,
        anchors: &BTreeSet<WindowId>,
    ) -> Result<()> {
        self.send_frame(
            placements,
            owners,
            stickies,
            None,
            Some(viewports),
            Some(NativePosition::Settled(anchors)),
        )
    }
    pub fn prepare_resize(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
        viewports: &BTreeMap<String, Rect>,
        window: WindowId,
        anchor: Rect,
    ) -> Result<()> {
        self.send_frame(
            placements,
            owners,
            stickies,
            None,
            Some(viewports),
            Some(NativePosition::Resize(window, anchor)),
        )
    }
    pub fn interactive_frame(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
        viewports: &BTreeMap<String, Rect>,
        window: WindowId,
        drag_frame: Option<Rect>,
    ) -> Result<()> {
        self.send_frame(
            placements,
            owners,
            stickies,
            Some((viewports, window, drag_frame)),
            Some(viewports),
            None,
        )
    }
    fn send_frame(
        &self,
        placements: &[Placement],
        owners: &BTreeMap<WindowId, i32>,
        stickies: &[StickyWindow],
        viewports: Option<(&BTreeMap<String, Rect>, WindowId, Option<Rect>)>,
        clip_viewports: Option<&BTreeMap<String, Rect>>,
        native: Option<NativePosition<'_>>,
    ) -> Result<()> {
        self.require_live_version()?;
        if placements.len() > 128 {
            bail!("Payload supports at most 128 controlled windows");
        }
        // Arm before sending: a rejected frame may have partially applied native state.
        self.armed.set(true);
        self.request(
            "frame",
            Some(
                placements
                    .iter()
                    .map(|p| {
                        Ok(Update {
                            wid: p.window.0,
                            pid: *owners
                                .get(&p.window)
                                .context("Missing expected window owner")?,
                            frame: p.frame,
                            clip: p.clip,
                            viewport: viewports
                                .filter(|(_, id, _)| *id == p.window)
                                .map(|(v, _, _)| {
                                    v.get(&p.monitor).copied().context("Missing drag viewport")
                                })
                                .transpose()?,
                            drag_frame: viewports
                                .filter(|(_, id, _)| *id == p.window)
                                .and_then(|(_, _, f)| f),
                            clip_viewport: clip_viewports.and_then(|v| v.get(&p.monitor).copied()),
                            anchor: matches!(&native, Some(NativePosition::Settled(ids)) if ids.contains(&p.window)),
                            native_frame: match native { Some(NativePosition::Resize(id, frame)) if id == p.window => Some(frame), _ => None },
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            Some(stickies),
            None,
        )?;
        Ok(())
    }
    pub fn reset(&self) -> Result<()> {
        self.request("reset", None, None, None)?;
        self.armed.set(false);
        Ok(())
    }
    pub fn overview(&self) -> Result<()> {
        self.request("overview", None, None, None)?;
        Ok(())
    }
    pub fn heartbeat(&self) -> Result<()> {
        self.request("heartbeat", None, None, None)?;
        Ok(())
    }
    pub fn detach(&self, placement: &Placement, pid: i32) -> Result<()> {
        self.request(
            "detach",
            Some(vec![Update {
                wid: placement.window.0,
                pid,
                frame: placement.frame,
                clip: Some(placement.frame),
                viewport: None,
                drag_frame: None,
                clip_viewport: None,
                anchor: false,
                native_frame: None,
            }]),
            None,
            None,
        )?;
        Ok(())
    }
    pub fn finish(&self, placements: &[Placement], owners: &BTreeMap<WindowId, i32>) -> Result<()> {
        let updates = placements
            .iter()
            .map(|p| {
                Ok(Update {
                    wid: p.window.0,
                    pid: *owners.get(&p.window).context("Missing release owner")?,
                    frame: p.frame,
                    clip: Some(p.frame),
                    viewport: None,
                    drag_frame: None,
                    clip_viewport: None,
                    anchor: false,
                    native_frame: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.request("finish", Some(updates), None, None)?;
        self.armed.set(false);
        Ok(())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        if self.armed.get()
            && let Err(e) = self.reset()
        {
            eprintln!("RibbonWM restore: {e:#}");
        }
    }
}
