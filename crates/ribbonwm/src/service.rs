//! Run the GUI service directly in Rust so TCC permission belongs to the WM.
use anyhow::{Context, Result, bail};
use ribbon_core::Settings;
use std::ffi::CStr;
use std::fs::{DirBuilder, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

struct PermissionProbe(Child);
impl PermissionProbe {
    fn spawn() -> Result<Self> {
        Ok(Self(
            Command::new(std::env::current_exe()?)
                .arg("permission-host")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?,
        ))
    }
    fn accessible(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
            && ribbon_macos::owned_probe_accessible(self.0.id() as i32)
    }
}
impl Drop for PermissionProbe {
    fn drop(&mut self) {
        self.0.stdin.take();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn permission_host() -> Result<()> {
    ribbon_macos::initialize_permission_host();
    loop {
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized descriptor; nonblocking check for parent EOF.
        if unsafe { libc::poll(&mut fd, 1, 0) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if fd.revents != 0 {
            return Ok(());
        }
        ribbon_macos::wait_for_events(0.1);
    }
}

pub fn run(user: String, config: PathBuf, exclude_apps: Vec<String>) -> Result<()> {
    // SAFETY: getuid has no arguments. This preflight runs on the main thread.
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        bail!("Run the WM as a login user, never root");
    }
    // SAFETY: getpwuid returns a static record, copied before any other lookup.
    let (name, home) = unsafe {
        let record = libc::getpwuid(uid);
        if record.is_null() {
            bail!("Cannot resolve login user");
        }
        (
            CStr::from_ptr((*record).pw_name)
                .to_string_lossy()
                .into_owned(),
            PathBuf::from(CStr::from_ptr((*record).pw_dir).to_string_lossy().as_ref()),
        )
    };
    if name != user {
        return Ok(()); // nix-darwin user agents can exist in other login sessions.
    }
    let logs = home.join("Library/Logs/RibbonWM");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&logs)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(logs.join("wm.log"))?;
    // SAFETY: dup2 duplicates the open log descriptor onto standard outputs.
    if unsafe { libc::dup2(log.as_raw_fd(), 1) < 0 || libc::dup2(log.as_raw_fd(), 2) < 0 } {
        return Err(std::io::Error::last_os_error().into());
    }
    let settings: Settings = toml::from_str(
        &std::fs::read_to_string(&config)
            .with_context(|| format!("Reading {}", config.display()))?,
    )?;
    settings.validate()?;
    let expected = ribbon_macos::backend::expected_build()?;
    eprintln!(
        "Starting service: executable={}, expected backend={expected:?}",
        std::env::current_exe()?.display()
    );
    if !ribbon_macos::accessibility_trusted() {
        eprintln!("Requesting Accessibility permission from macOS");
        ribbon_macos::request_accessibility_permission();
    }
    if settings.gesture_scroll && !ribbon_macos::input::trusted() {
        eprintln!("Requesting Input Monitoring permission for configured gestures");
        ribbon_macos::input::request_permission();
    }
    let mut last_readiness = None;
    let mut probe = None;
    loop {
        let mut accessibility = ribbon_macos::accessibility_trusted();
        if !accessibility {
            if probe.is_none() {
                probe = Some(PermissionProbe::spawn()?);
            }
            accessibility = probe.as_mut().is_some_and(PermissionProbe::accessible);
        }
        let status = ribbon_macos::backend::Backend::connect().and_then(|b| b.status());
        let backend = status.as_ref().is_ok_and(|s| {
            s.version == 2
                && s.capabilities.iter().any(|c| c == "sticky")
                && s.capabilities.iter().any(|c| c == "interactive_clip")
                && s.capabilities.iter().any(|c| c == "overview")
                && s.capabilities.iter().any(|c| c == "finish")
                && s.capabilities.iter().any(|c| c == "pointer_drag")
                && s.capabilities.iter().any(|c| c == "window_groups")
                && expected.as_ref().is_none_or(|build| *build == s.build)
        });
        if accessibility && backend {
            break;
        }
        if last_readiness != Some((accessibility, backend)) {
            eprintln!(
                "Waiting: Accessibility={accessibility}, sticky backend={backend}; grant Accessibility to {}",
                std::env::current_exe()?.display()
            );
            if !backend {
                eprintln!("Backend preflight: {status:?}; expected {expected:?}");
            }
            last_readiness = Some((accessibility, backend));
        }
        ribbon_macos::wait_for_events(0.25);
    }
    drop(probe);
    eprintln!(
        "Permissions/backend ready in service pid {}; continuing without relaunch",
        std::process::id()
    );
    crate::daemon::run_ready(crate::daemon::Options {
        selected: Vec::new(),
        all: true,
        exclude_apps,
        dry_run: false,
        settings,
    })
}
