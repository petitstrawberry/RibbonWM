use anyhow::{Context, Result, bail};
use ribbon_core::{Action, WindowId};
use serde::{Deserialize, Serialize};
use std::fs::{self, Permissions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const LIMIT: usize = 65536;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    SetMode {
        window: Option<WindowId>,
        kind: crate::modes::ModeKind,
        enabled: Option<bool>,
    },
    FocusMonitor {
        target: String,
    },
    Configure {
        name: String,
        value: Option<f64>,
    },
    Status {},
    Apply {
        monitor: Option<String>,
        action: Action,
    },
    FocusWindow {
        window: WindowId,
    },
    Quit {},
}
pub fn socket_path() -> PathBuf {
    PathBuf::from(format!(
        "/tmp/ribbonwm-{}/wm.sock",
        ribbon_macos::backend::uid()
    ))
}

pub struct SocketLease {
    pub listener: UnixListener,
    file: PathBuf,
    inode: u64,
}
impl SocketLease {
    pub fn bind() -> Result<Self> {
        let file = socket_path();
        let directory = file.parent().context("Invalid IPC directory")?;
        match fs::create_dir(directory) {
            Ok(()) => fs::set_permissions(directory, Permissions::from_mode(0o700))?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let meta = fs::symlink_metadata(directory)?;
        if !meta.is_dir()
            || meta.uid() != ribbon_macos::backend::uid()
            || meta.mode() & 0o777 != 0o700
        {
            bail!("IPC directory must be a real directory owned by this user with mode 0700");
        }
        if let Ok(meta) = fs::symlink_metadata(&file) {
            if !meta.file_type().is_socket() || meta.uid() != ribbon_macos::backend::uid() {
                bail!("Refusing to replace unexpected IPC path");
            }
            match UnixStream::connect(&file) {
                Ok(_) => bail!("RibbonWM is already running"),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(&file)?
                }
                Err(e) => return Err(e.into()),
            }
        }
        let listener = UnixListener::bind(&file)?;
        fs::set_permissions(&file, Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let inode = fs::symlink_metadata(&file)?.ino();
        Ok(Self {
            listener,
            file,
            inode,
        })
    }
}
impl Drop for SocketLease {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.file)
            .is_ok_and(|m| m.ino() == self.inode && m.file_type().is_socket())
        {
            let _ = fs::remove_file(&self.file);
        }
    }
}
fn check_peer(stream: &UnixStream) -> Result<()> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: fd belongs to a connected UnixStream; uid/gid point to initialized writable storage.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 || uid != ribbon_macos::backend::uid() {
        bail!("IPC peer is not this user");
    }
    Ok(())
}
fn read_frame(stream: &mut UnixStream, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("IPC deadline exceeded")?;
        stream.set_read_timeout(Some(remaining))?;
        let count = match stream.read(&mut chunk) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            bail!("Incomplete IPC message");
        }
        if data.len() + count >= LIMIT {
            bail!("IPC message too large");
        }
        data.extend_from_slice(&chunk[..count]);
        if let Some(index) = data.iter().position(|b| *b == b'\n') {
            data.truncate(index);
            return Ok(data);
        }
    }
}
pub fn receive(stream: &mut UnixStream) -> Result<Request> {
    check_peer(stream)?;
    Ok(serde_json::from_slice(&read_frame(
        stream,
        Duration::from_millis(200),
    )?)?)
}
pub fn respond(stream: &mut UnixStream, value: &serde_json::Value) -> Result<()> {
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let mut data = serde_json::to_vec(value)?;
    if data.len() >= LIMIT {
        bail!("IPC response too large");
    }
    data.push(b'\n');
    stream.write_all(&data)?;
    Ok(())
}
pub fn send(request: Request) -> Result<serde_json::Value> {
    let mut stream =
        UnixStream::connect(socket_path()).context("RibbonWM is not running; start run first")?;
    check_peer(&stream)?;
    respond(&mut stream, &serde_json::to_value(request)?)?;
    let value: serde_json::Value =
        serde_json::from_slice(&read_frame(&mut stream, Duration::from_secs(2))?)?;
    if value["ok"] != true {
        bail!(
            "{}",
            value["error"].as_str().unwrap_or("IPC command rejected")
        );
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_requests_are_reassembled() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            a.write_all(b"{\"op\":").unwrap();
            std::thread::sleep(Duration::from_millis(10));
            a.write_all(b"\"status\"}\n").unwrap();
        });
        assert!(matches!(receive(&mut b).unwrap(), Request::Status {}));
        writer.join().unwrap();
    }
    #[test]
    fn invalid_and_oversized_requests_are_rejected() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(b"{\"op\":\"status\",\"unexpected\":1}\n")
            .unwrap();
        assert!(receive(&mut b).is_err());
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(
            b"{\"op\":\"apply\",\"action\":{\"action\":\"stack_left\",\"unexpected\":1}}\n",
        )
        .unwrap();
        assert!(receive(&mut b).is_err());
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            let _ = a.write_all(&vec![b'x'; LIMIT]);
        });
        assert!(receive(&mut b).is_err());
        drop(b);
        writer.join().unwrap();
    }
    #[test]
    fn idle_connections_cannot_block_forever() {
        let (_a, mut b) = UnixStream::pair().unwrap();
        let started = Instant::now();
        assert!(receive(&mut b).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
