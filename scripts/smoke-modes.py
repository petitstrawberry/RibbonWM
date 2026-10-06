"""Float/sticky and restoration against one owned, shell-free Alacritty."""
import json
import os
from pathlib import Path
import runpy
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "native/build/test-window"
support = runpy.run_path(str(ROOT / "scripts/smoke-live.py"))
inspect = support["inspect"]


def main(binary, alacritty):
    assert os.environ.get("IN_NIX_SHELL")
    isolated = "--isolated" in sys.argv[3:]
    daemon = None
    log = None
    if isolated:
        assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    def cli(*args):
        return json.loads(subprocess.check_output([binary, *map(str, args)], text=True, timeout=5))
    def wait(predicate, reason, timeout=8):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            value = predicate()
            if value:
                return value
            time.sleep(.08)
        raise RuntimeError(reason)
    def inventory():
        return next(w for w in cli("windows") if w["id"] == wid and w["pid"] == p.pid)
    def mode():
        return cli("status")["window_modes"][str(wid)]
    def tiled():
        return any(plan["window"] == wid for plan in cli("status")["placements"])
    def space():
        return next(d["native_space"] for d in cli("displays") if d["id"] == display["id"])
    def native_focus():
        subprocess.run([str(HELPER), "--focus-fixture", str(wid), str(p.pid)], check=True, timeout=3)
    assert "sticky" in cli("backend-status")["capabilities"]
    display = next(d for d in cli("displays") if d["primary"])
    settings=cli("status")["state"]["settings"] if not isolated else dict(padding_top=24, padding_bottom=24)
    top=settings["padding_top"] if settings["padding_top"] is not None else settings["vertical_margin"]
    bottom=settings["padding_bottom"] if settings["padding_bottom"] is not None else settings["vertical_margin"]
    tiled_height=display["viewport"]["height"]-top-bottom
    print("Mode test display:",display,"tile height:",tiled_height,flush=True)
    original_space = display["native_space"]
    user_space = original_space
    with tempfile.TemporaryDirectory(prefix="ribbon-mode-input-") as temporary:
        keyfile = Path(temporary) / "input.json"
        p = subprocess.Popen([alacritty, "--title", "RibbonWM QA Modes 0", "-o", "window.dimensions.columns=120",
                              "-o", "window.dimensions.lines=30", "-o", "window.position.x=200", "-o", "window.position.y=260",
                              "-e", sys.executable, str(ROOT / "scripts/smoke-live.py"), "--terminal-child", "0", str(keyfile)],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            raw = wait(lambda: next((w for w in cli("windows") if w["pid"] == p.pid and w["title"].startswith("RibbonWM QA Modes ")), None), "Fixture window not created")
            wid = raw["id"]
            if isolated:
                native_focus()
                time.sleep(.5)
                print("Before management:", raw, inspect(wid), flush=True)
                config = Path(temporary) / "config.toml"
                config.write_text("padding_top=24.0\npadding_bottom=24.0\npadding_left=24.0\npadding_right=24.0\n"
                                  "gap=6.0\npreserve_window_width=true\ncenter_content=true\nanimation_duration=0.1\n")
                with config.open("a") as stream:
                    stream.write("animation_curve='ease_out'\n")
                log = (ROOT / "docs/modes-daemon.txt").open("w")
                daemon = subprocess.Popen([binary, "run", "--windows", str(wid), "--config", str(config),
                    "--exclude-app", "com.openai.*", "--exclude-app", "ChatGPT*",
                    "--exclude-app", "com.apple.systempreferences"], stdout=log, stderr=log)
                wait(lambda: Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists(), "Owned daemon did not start")
            # App restoration may open the new process on its previous Space.
            # Activate only our owned fixture before requiring visible enrollment.
            native_focus()
            def enrolled():
                originals = cli("status")["original_geometry"]
                return next((w for w in cli("windows") if w["pid"] == p.pid and str(w["id"]) in originals), None)
            window = wait(enrolled, "Fixture not automatically enrolled")
            wid = window["id"]
            original_space = space()
            native_focus()
            wait(lambda: abs(next(x["frame"]["width"] for x in cli("status")["placements"] if x["window"]==wid)-inspect(wid)["frame"]["width"])<2 and abs(inspect(wid)["frame"]["height"]-tiled_height)<2, "Initial tile did not settle")
            time.sleep(.3)
            baseline=next(x["frame"] for x in cli("status")["placements"] if x["window"]==wid)
            cli("-m", "window", "--toggle", "float")
            wait(lambda: mode()["floating"] and not tiled(), "Float did not leave the strip")
            time.sleep(.5)
            restored = inspect(wid)
            assert all(abs(-restored["transform"][4+i]-baseline[k])<2 for i,k in enumerate(("x","y"))), (restored, baseline)
            assert restored["transform"][:4] == [1, 0, 0, 1], restored
            assert restored["clip_bounds"][:2] == [0, 0], restored
            assert abs(restored["clip_bounds"][2]-baseline["width"]) <= 2 and abs(restored["clip_bounds"][3]-baseline["height"]) <= 2, (restored, baseline)
            width, height = baseline["width"] + 120, baseline["height"] - 120
            subprocess.run([str(HELPER), "--resize-fixture", str(wid), str(p.pid), str(width), str(height)], check=True, timeout=3)
            time.sleep(1.2)
            cli("float", "on", "--window", wid)
            assert not tiled() and abs(inspect(wid)["frame"]["width"] - width) < 2
            assert abs(inspect(wid)["frame"]["height"] - height) < 2
            cli("float", "off", "--window", wid)
            wait(lambda: tiled() and abs(inspect(wid)["frame"]["height"] - tiled_height) < 2, "Retile not applied")
            assert abs(inspect(wid)["frame"]["width"] - width) < 2
            print("PASS: float releases real geometry and input, survives inventory polls/manual resize, retile adopts the new width", flush=True)
            if "--float-only" in sys.argv[3:]:
                for _ in range(10):
                    time.sleep(.3)
                    before=next(x["frame"] for x in cli("status")["placements"] if x["window"]==wid)
                    cli("float", "on", "--window", wid)
                    wait(lambda: not tiled(), "Repeated detach failed")
                    time.sleep(.15)
                    surface=inspect(wid)
                    assert surface["clip_bounds"][:2] == [0,0], surface
                    assert abs(surface["clip_bounds"][2]-width)<2 and abs(surface["clip_bounds"][3]-tiled_height)<2, surface
                    assert all(abs(-surface["transform"][4+i]-before[k])<2 for i,k in enumerate(("x","y"))), (surface,before)
                    cli("float", "off", "--window", wid)
                    wait(lambda: tiled() and abs(inspect(wid)["frame"]["height"]-tiled_height)<2, "Repeated retile failed")
                print("PASS: ten detach/retile cycles preserve floating position, physical size and full clip", flush=True)
                return
            cli("float", "on", "--window", wid)
            cli("sticky", "on", "--window", wid)
            wait(lambda: inventory()["sticky"] and not tiled(), "Native sticky tag not applied")
            assert mode()["floating"] and mode()["sticky"]
            # Click only the named fixture; position the pointer on this display
            # before using the user's enabled native Space shortcut.
            native_focus()
            subprocess.run([str(HELPER), "--click-fixture", str(wid), str(p.pid)], check=True, timeout=3)
            before = inspect(wid)
            subprocess.run([str(HELPER), "--space", "next"], check=True)
            wait(lambda: space() != original_space, "Native Space did not change")
            destination_space = space()
            wait(lambda: inventory()["onscreen"] and inventory()["sticky"], "Sticky fixture disappeared on next Space")
            # Space metadata changes before Mission Control finishes its slide.
            time.sleep(.9)
            # macOS activates the destination Space's app above normal-level
            # sticky windows. Explicitly focus our fixture through the public
            # command before checking input; sticky is not an always-on-top flag.
            cli("-m", "window", "--focus", wid)
            count = json.loads(keyfile.read_text())["mouse"]
            subprocess.run([str(HELPER), "--click-fixture", str(wid), str(p.pid)], check=True, timeout=3)
            wait(lambda: json.loads(keyfile.read_text())["mouse"] > count, "Sticky fixture did not receive real input on next Space")
            assert inspect(wid)["frame"] == before["frame"]
            assert not tiled()
            cli("sticky", "off", "--window", wid)
            wait(lambda: not inventory()["sticky"], "Sticky tag was not cleared")
            print("After clearing sticky:", {k: inventory()[k] for k in ("id", "onscreen", "sticky", "native_spaces")}, "destination:", destination_space, flush=True)
            assert inventory()["onscreen"], inventory()
            assert inventory()["native_spaces"] == [destination_space], inventory()
            assert mode()["floating"] and not mode()["sticky"] and not tiled()
            native_focus()
            cli("sticky", "on", "--window", wid)
            cli("-m", "window", "--toggle", "sticky")
            assert mode()["floating"] and not mode()["sticky"] and not tiled()
            cli("float", "off", "--window", wid)
            wait(tiled, "Removing the last detached flag did not retile")
            placement = next(p for p in cli("status")["placements"] if p["window"] == wid)
            assert placement["native_space"] == destination_space and placement["monitor"] == display["id"], placement
            cli("sticky", "on", "--window", wid)
            cli("float", "off", "--window", wid)
            assert mode()["sticky"] and not mode()["floating"] and not tiled()
            cli("sticky", "off", "--window", wid)
            wait(tiled, "Sticky-only window did not retile")
            queried = next(w for w in cli("-m", "query", "--windows") if w["id"] == wid)
            assert queried["is-tiled"] and not queried["is-floating"] and not queried["is-sticky"]
            print("PASS: sticky stays onscreen across Spaces and receives terminal input after explicit focus; clearing it remains visible and preserves independent float", flush=True)
            cli("sticky", "on", "--window", wid)
            cli("quit")
            wait(lambda: not inventory()["sticky"] and cli("backend-status")["controlled"] == 0, "Normal exit did not restore sticky")
            print("PASS: normal WM exit restores the original sticky tag and releases the compositor lease", flush=True)
            def request(body):
                body["session"] = "mode-watchdog-" + str(os.getpid())
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                    client.settimeout(1)
                    client.connect(f"/tmp/ribbonwm-{os.getuid()}/backend.sock")
                    client.sendall(json.dumps(body).encode() + b"\n")
                    return json.loads(client.makefile("rb").readline())
            assert not request(dict(op="sticky", window=dict(wid=wid, pid=p.pid+1, enabled=True)))["ok"]
            assert not inventory()["sticky"]
            assert request(dict(op="sticky", window=dict(wid=2**32-1, pid=p.pid, enabled=True)))["error"] == "Sticky owner changed"
            for malformed in (True, 0, 1.5, 2**32):
                assert request(dict(op="sticky", window=dict(wid=malformed, pid=p.pid, enabled=True)))["error"] == "Invalid sticky descriptor"
            assert request(dict(op="sticky", window=dict(wid=wid, pid=p.pid, enabled=True)))["ok"]
            assert inventory()["sticky"]
            wait(lambda: not inventory()["sticky"] and cli("backend-status")["controlled"] == 0, "Sticky watchdog did not restore", timeout=4)
            print("PASS: full u32 IDs are parsed, malformed IDs/mismatched owners are rejected, watchdog restores the original tag without a controller", flush=True)
        finally:
            # Restore the WM before traversing any Spaces for cleanup, including
            # an early assertion failure. Do not enroll unrelated apps en route.
            if subprocess.run([binary, "status"], capture_output=True, timeout=5).returncode == 0:
                cli("quit")
                wait(lambda: cli("backend-status")["controlled"] == 0, "WM cleanup did not release its lease")
            if p.poll() is None:
                p.terminate()
                p.wait(timeout=5)
            if daemon is not None:
                daemon.wait(timeout=5)
            if log is not None:
                log.close()
            # Closing the focused app can itself activate another native Space.
            time.sleep(1)
            snapshot = json.loads(subprocess.check_output([str(HELPER), "--spaces"], text=True))
            row = next(d for d in snapshot["displays"] if d["Display Identifier"] == display["id"])
            order = [s["ManagedSpaceID"] for s in row["Spaces"]]
            for _ in order:
                current = space()
                if current == user_space:
                    break
                index = order.index(current)
                step = 1 if index < order.index(user_space) else -1
                target = order[index + step]
                subprocess.run([str(HELPER), "--space", "next" if step == 1 else "previous"], check=True)
                wait(lambda: space() == target, "Native Space restoration did not settle")
                time.sleep(.8)
            assert space() == user_space, "QA did not return to the user's original Space"


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
