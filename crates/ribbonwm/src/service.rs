//! Run the GUI service directly in Rust so TCC permission belongs to the WM.
use anyhow::{Context, Result, bail};
use ribbon_core::Settings;
use std::ffi::CStr;
use std::fs::{DirBuilder, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Duration;

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
    loop {
        let accessibility = ribbon_macos::accessibility_trusted();
        let status = ribbon_macos::backend::Backend::connect().and_then(|b| b.status());
        let backend = status.as_ref().is_ok_and(|s| {
            s.version == 2
                && s.capabilities.iter().any(|c| c == "sticky")
                && expected.as_ref().is_none_or(|build| *build == s.build)
        });
        if accessibility && backend {
            break;
        }
        eprintln!(
            "Waiting: Accessibility={accessibility}, sticky backend={backend}; grant Accessibility to {}",
            std::env::current_exe()?.display()
        );
        if !backend {
            eprintln!("Backend preflight: {status:?}; expected {expected:?}");
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    crate::daemon::run(crate::daemon::Options {
        selected: Vec::new(),
        all: true,
        exclude_apps,
        dry_run: false,
        settings,
    })
}
