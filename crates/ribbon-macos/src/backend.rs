//! Bounded same-user Unix-socket client for the optional arm64e Dock payload.
use anyhow::{Context, Result, bail};
use ribbon_core::{Placement, Rect, WindowId};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::BTreeMap;
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
#[derive(Serialize)]
struct Update {
    wid: u32,
    pid: i32,
    frame: Rect,
    clip: Option<Rect>,
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

pub struct Backend {
    session: String,
    armed: Cell<bool>,
    version: Cell<u32>,
    sticky: Cell<bool>,
}
impl Backend {
    pub fn connect() -> Result<Self> {
        let b = Self {
            armed: Cell::new(false),
            version: Cell::new(0),
            sticky: Cell::new(false),
            session: format!(
                "{}-{}-{}",
                uid(),
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ),
        };
        let status = b.status()?;
        if !matches!(status.version, 1 | 2) || status.uid != uid() {
            bail!("Unexpected payload version or user");
        }
        b.version.set(status.version);
        b.sticky
            .set(status.capabilities.iter().any(|s| s == "sticky"));
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
        if self.version.get() != 2 {
            bail!(
                "Reload the Dock backend with nix develop -c sh scripts/load-backend.sh (protocol 2 required)"
            );
        }
        Ok(())
    }
    pub fn frame(&self, placements: &[Placement], owners: &BTreeMap<WindowId, i32>) -> Result<()> {
        self.frame_with_sticky(placements, owners, &[])
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
