#!/bin/sh
set -eu
if [ -z "${IN_NIX_SHELL:-}" ]; then
    echo 'Run: nix develop -c sh scripts/load-backend.sh' >&2
    exit 1
fi
ribbon_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
make -C "$ribbon_root/native"
cargo build --locked --manifest-path "$ribbon_root/Cargo.toml"
# dlopen of an already loaded path does not run its constructor again. Every
# build gets a distinct, byte-identical signed image path. Refuse active leases.
ribbon_payload=$(python3 - "$ribbon_root" <<'PY'
import hashlib, json, os, shutil, socket, sys
from pathlib import Path
root = Path(sys.argv[1])
runtime = Path(f'/tmp/ribbonwm-{os.getuid()}')
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
    probe.settimeout(0.5)
    try:
        probe.connect(str(runtime / 'wm.sock'))
    except (FileNotFoundError, ConnectionRefusedError):
        pass
    else:
        raise SystemExit('Stop RibbonWM before replacing its backend: target/debug/ribbonwm quit')
source = root / 'native/build/ribbon-payload.dylib'
target = source.with_name('ribbon-payload-' + hashlib.sha256(source.read_bytes()).hexdigest()[:20] + '.dylib')
if not target.exists():
    shutil.copyfile(source, target)
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
    probe.settimeout(0.5)
    try:
        probe.connect(str(runtime / 'backend.sock'))
    except (FileNotFoundError, ConnectionRefusedError):
        print(target)
    else:
        probe.sendall(b'{"op":"hello"}\n')
        response = probe.makefile('rb').readline(65536)
        status = json.loads(response)
        if status.get('uid') != os.getuid() or status.get('controlled') != 0:
            raise SystemExit('Backend has an active lease or unexpected owner')
        print('already-loaded' if status.get('version') == 2 and status.get('build') == target.name else target)
PY
)
if [ "$ribbon_payload" != already-loaded ]; then
    sudo "$ribbon_root/native/build/dock-loader" "$ribbon_payload"
fi
cargo run --manifest-path "$ribbon_root/Cargo.toml" -- backend-status
python3 - "$ribbon_root" "$ribbon_payload" <<'PY'
import json, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
status = json.loads(subprocess.check_output([root / 'target/debug/ribbonwm', 'backend-status']))
if status['version'] != 2 or 'sticky' not in status.get('capabilities', []) or (sys.argv[2] != 'already-loaded' and status.get('build') != Path(sys.argv[2]).name):
    raise SystemExit('New backend did not load; the old backend was left idle. Check Dock loader diagnostics.')
PY
