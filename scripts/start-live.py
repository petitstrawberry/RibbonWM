"""Start an explicitly requested live session, detached from this shell.

Run through nix develop. Stop with: target/debug/ribbonwm quit
Does not register a login service or change SIP or another WM's service.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/debug/ribbonwm"
RUNTIME = Path(f"/tmp/ribbonwm-{os.getuid()}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exclude-app", action="append", default=[])
    parser.add_argument("--config", type=Path, default=ROOT / "config/live.toml")
    args = parser.parse_args()
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run through nix develop -c python3 scripts/start-live.py")
    config = args.config.resolve(strict=True)
    subprocess.run(["cargo", "build", "--locked", "--workspace"], cwd=ROOT, check=True)
    doctor = json.loads(subprocess.check_output([str(BINARY), "doctor"], text=True, timeout=5))
    if not doctor["accessibility"] or not doctor["backend"]["available"]:
        raise RuntimeError("Accessibility permission and the loaded Dock backend are required")
    if doctor["backend"]["status"]["version"] != 2:
        raise RuntimeError("Reload the Dock backend: nix develop -c sh scripts/load-backend.sh")
    if doctor["backend"]["status"]["controlled"]:
        raise RuntimeError("Another session already controls windows")
    metadata = RUNTIME.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o777 != 0o700:
        raise RuntimeError("Unexpected runtime directory ownership or permissions")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.5)
        try:
            probe.connect(str(RUNTIME / "wm.sock"))
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        else:
            raise RuntimeError("RibbonWM is already running")
    log = RUNTIME / "wm.log"
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    os.fchmod(fd, 0o600)
    command = [str(BINARY), "run", "--all", "--config", str(config)]
    for excluded in args.exclude_app:
        command.extend(["--exclude-app", excluded])
    try:
        process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL,
                                   stdout=fd, stderr=fd, start_new_session=True)
    finally:
        os.close(fd)
    try:
        deadline = time.monotonic() + 10
        ready_polls = 0
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"Live daemon exited: {log.read_text()}")
            result = subprocess.run([str(BINARY), "status"], capture_output=True, text=True, timeout=4)
            if result.returncode == 0:
                value = json.loads(result.stdout)
                if value.get("mode") != "live":
                    raise RuntimeError("Unexpected daemon mode")
                monitors = value["state"]["monitors"].values()
                managed = {wid for m in monitors for row in m["contexts"].values()
                           for column in row["columns"] for wid in column["windows"]}
                active_scrolls = [m["contexts"][str(m["native_space"])]["scroll"]
                                  for m in value["state"]["monitors"].values() if not m["suspended"]]
                backend = json.loads(subprocess.check_output([str(BINARY), "backend-status"], text=True, timeout=3))
                ready = backend["controlled"] == len(managed) and all(
                    abs(s["position"]-s["target"]) < 0.01 and abs(s["velocity"]) < 0.01
                    for s in active_scrolls)
                ready_polls = ready_polls + 1 if ready else 0
                if ready_polls >= 2:
                    print(json.dumps(dict(pid=process.pid, log=str(log), status=value), ensure_ascii=False))
                    return
            time.sleep(0.1)
        raise RuntimeError(f"Daemon startup did not complete; see {log}")
    except BaseException:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        raise


if __name__ == "__main__":
    main()
