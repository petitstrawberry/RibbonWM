"""Fixed packaged Dock loader, probing the backend as the selected login user."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import pwd
import socket
import stat
import subprocess
import sys
import time

DOCK = "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock"


def probe():
    runtime = Path(f"/tmp/ribbonwm-{os.getuid()}")
    if runtime.exists():
        metadata = runtime.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
            raise RuntimeError("Unexpected runtime directory")
    result = {"wm_running": False, "backend": None}
    for name in ("wm.sock", "backend.sock"):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(.5)
            try:
                client.connect(str(runtime / name))
            except (FileNotFoundError, ConnectionRefusedError):
                continue
            if name == "wm.sock":
                result["wm_running"] = True
                client.sendall(b'{"op":"status"}\n')
                try:
                    client.makefile("rb").readline(65536)
                except TimeoutError:
                    pass
            else:
                libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
                libc.getpeereid.argtypes = [ctypes.c_int, ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
                libc.getpeereid.restype = ctypes.c_int
                uid, gid = ctypes.c_uint(), ctypes.c_uint()
                if libc.getpeereid(client.fileno(), ctypes.byref(uid), ctypes.byref(gid)) != 0 or uid.value != os.getuid():
                    raise RuntimeError("Unexpected backend peer")
                client.sendall(b'{"op":"hello"}\n')
                result["backend"] = json.loads(client.makefile("rb").readline(65536))
    print(json.dumps(result))


def expected_backend(backend, payload, uid, dock):
    return bool(backend and backend.get("ok") and backend.get("version") == 2
                and backend.get("build") == payload.name and backend.get("uid") == uid
                and backend.get("pid") == dock)


def wait_for_backend(command, user, payload, dock, timeout=2):
    # dlopen returns before the payload's listener necessarily starts serving.
    deadline = time.monotonic() + timeout
    loaded = None
    while True:
        result = subprocess.run(command, user=user.pw_uid, group=user.pw_gid, extra_groups=[],
                                capture_output=True, text=True, timeout=3, check=True)
        loaded = json.loads(result.stdout)["backend"]
        if expected_backend(loaded, payload, user.pw_uid, dock):
            return loaded
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Backend identity mismatch: expected {payload.name}, Dock {dock}; received {loaded}")
        time.sleep(.05)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--watch", action="store_true")
    parser.add_argument("--user")
    parser.add_argument("--loader")
    parser.add_argument("--payload")
    args = parser.parse_args()
    if args.probe:
        probe()
        return
    if os.geteuid() != 0 or not all((args.user, args.loader, args.payload)):
        raise SystemExit("Use sudo ribbonwm-load-backend --user LOGIN_USER, or enableDockInjection")
    user = pwd.getpwnam(args.user)
    if user.pw_uid == 0:
        raise SystemExit("A non-root login user is required")
    payload = Path(args.payload).resolve(strict=True)
    while True:
        try:
            rows = subprocess.check_output(["/bin/ps", "-axo", "pid=,uid=,comm="], text=True)
            docks = [int(parts[0]) for line in rows.splitlines() if len(parts := line.strip().split(None, 2)) == 3
                     and parts[1] == str(user.pw_uid) and parts[2] == DOCK]
            if len(docks) != 1:
                raise RuntimeError("Waiting for one Dock belonging to the configured user")
            command = [sys.executable, str(Path(__file__).resolve()), "--probe"]
            result = subprocess.run(command, user=user.pw_uid, group=user.pw_gid, extra_groups=[],
                                    capture_output=True, text=True, timeout=3, check=True)
            state = json.loads(result.stdout)
            old = state["backend"]
            if expected_backend(old, payload, user.pw_uid, docks[0]):
                if not args.watch:
                    print("Dock backend already loaded", flush=True)
                    return
            elif state["wm_running"] or (old and old.get("controlled", 0)):
                raise RuntimeError("Stop RibbonWM before replacing the backend")
            else:
                subprocess.run([args.loader, str(payload), str(docks[0])], check=True, timeout=10)
                wait_for_backend(command, user, payload, docks[0])
                print(f"Dock backend ready for {args.user}, pid {docks[0]}", flush=True)
                if not args.watch:
                    return
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
            if not args.watch:
                raise SystemExit(str(error)) from error
            print(str(error), file=sys.stderr, flush=True)
        time.sleep(5)


if __name__ == "__main__":
    main()
