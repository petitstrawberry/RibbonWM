"""Read actual desktop metadata; exercise daemon/CLI without writing app geometry."""
import json
import os
from pathlib import Path
import subprocess
import time

if not os.environ.get("IN_NIX_SHELL"):
    raise SystemExit("Run: nix develop -c python3 scripts/smoke-dry-run.py")
root = Path(__file__).resolve().parents[1]
subprocess.run(["cargo", "build", "--locked", "--workspace"], cwd=root, check=True)
binary = root / "target/debug/ribbonwm"
socket = Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock")
if socket.exists():
    raise SystemExit("An existing daemon socket is present; finish that run first.")


def cli(*args):
    result = subprocess.run([str(binary), *args], cwd=root, capture_output=True, text=True, timeout=5)
    if result.returncode:
        raise RuntimeError(f"{args}: {result.stderr.strip()}")
    value = json.loads(result.stdout)
    if isinstance(value, dict) and "ok" in value:
        assert value["ok"], value
    return value


displays = cli("displays")
inventory = cli("windows")
chosen = []
for window in inventory:
    b = window["bounds"]
    if window["layer"] != 0 or not window["onscreen"] or b["width"] < 100 or b["height"] < 100:
        continue
    if any(not d["native_fullscreen"] and window["native_spaces"] == [d["native_space"]]
           and min(b["x"] + b["width"], d["frame"]["x"] + d["frame"]["width"]) > max(b["x"], d["frame"]["x"])
           and min(b["y"] + b["height"], d["frame"]["y"] + d["frame"]["height"]) > max(b["y"], d["frame"]["y"])
           for d in displays):
        chosen.append(window["id"])
    if len(chosen) == 3:
        break
if not chosen:
    raise SystemExit("No ordinary window in the current desktop context is available.")
daemon = subprocess.Popen([str(binary), "run", "--dry-run", "--windows", ",".join(map(str, chosen))],
                          cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def verify():
    state = cli("status")
    assert state["mode"] == "dry_run"
    seen = []
    for monitor in state["state"]["monitors"].values():
        for context in monitor["contexts"].values():
            for column in context["columns"]:
                seen.extend(column["windows"])
    assert sorted(seen) == sorted(chosen), "Window identities lost or duplicated"


try:
    deadline = time.monotonic() + 5
    while not socket.exists():
        if daemon.poll() is not None:
            raise RuntimeError(daemon.communicate()[1])
        if time.monotonic() > deadline:
            raise TimeoutError("Daemon did not bind its socket")
        time.sleep(0.025)
    verify()
    # Select each window's monitor explicitly so pointer location cannot affect the scenario.
    state = cli("status")["state"]
    for monitor_id, monitor in state["monitors"].items():
        ids = [wid for context in monitor["contexts"].values() for column in context["columns"] for wid in column["windows"]]
        if not ids:
            continue
        cli("focus-window", str(ids[-1]))
        for command in [("focus", "left"), ("focus", "right"), ("scroll", "120"),
                        ("resize", "800"), ("stack",), ("focus", "down"), ("unstack",)]:
            cli("--monitor", monitor_id, *command)
            verify()
    # Restoration of the logical original row through window identity.
    cli("focus-window", str(chosen[0]))
    verify()
    cli("quit")
    daemon.communicate(timeout=5)
    assert daemon.returncode == 0
    assert not socket.exists(), "Daemon left its socket behind"
    print(f"PASS: dry-run CLI, identity/row invariants, graceful quit; {len(chosen)} windows, {len(displays)} displays")
finally:
    if daemon.poll() is None:
        daemon.terminate()
        daemon.communicate(timeout=5)
