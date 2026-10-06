"""Owned windows only: clipped sizes, app activation and real native Space return.

Run with no existing WM controller, via nix develop. Never enrolls user apps.
"""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "native/build/test-window"


def main(binary, alacritty):
    assert os.environ.get("IN_NIX_SHELL")
    assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    children, daemon = [], None

    def cli(*args):
        return json.loads(subprocess.check_output([binary, *map(str, args)], text=True, timeout=5))

    display = next(d for d in cli("displays") if d["primary"])
    monitor, original_space = display["id"], display["native_space"]
    row = next(d for d in json.loads(subprocess.check_output([str(HELPER), "--spaces"]))["displays"]
               if d["Display Identifier"] == monitor)
    order = [s["ManagedSpaceID"] for s in row["Spaces"]]
    index = order.index(original_space)
    direction = "next" if index + 1 < len(order) else "previous"
    other = order[index + (1 if direction == "next" else -1)]
    assert next(s for s in row["Spaces"] if s["ManagedSpaceID"] == other)["type"] == 0
    reverse = "previous" if direction == "next" else "next"

    def wait(predicate, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert daemon.poll() is None, "WM exited; see docs/space-retention-daemon.txt"
            try:
                value = predicate(cli("status"))
                if value:
                    return value
            except subprocess.CalledProcessError:
                pass
            time.sleep(.025)
        raise AssertionError("Owned WM state did not converge")

    def layout(state):
        return state["state"]["monitors"][monitor]["contexts"][str(original_space)]

    def settled():
        return wait(lambda s: s if abs(layout(s)["scroll"]["position"] - layout(s)["scroll"]["target"]) < .01 else None)

    def native_space():
        return next(d["native_space"] for d in cli("displays") if d["id"] == monitor)

    def switch(step, target):
        subprocess.run([str(HELPER), "--space", step], check=True, timeout=3)
        wait(lambda s: s["state"]["monitors"][monitor]["native_space"] == target)
        time.sleep(.7)

    with tempfile.TemporaryDirectory(prefix="ribbon-space-retention-") as temporary:
        config = Path(temporary) / "config.toml"
        config.write_text("padding_top=24.0\npadding_bottom=24.0\npadding_left=24.0\npadding_right=24.0\n"
                          "gap=6.0\npreserve_window_width=true\ncenter_content=true\nframe_rate=120\n"
                          "animation_curve='ease_out'\nanimation_duration=0.10\n")
        try:
            for i in range(5):
                children.append(subprocess.Popen([alacritty, "--title", f"RibbonWM QA Space Retention {i}",
                    "-o", "window.dimensions.columns=120", "-o", "window.dimensions.lines=25",
                    "-e", sys.executable, "-c", "import time; time.sleep(120)"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
                time.sleep(.15)
            deadline = time.monotonic() + 10
            while True:
                windows = [w for w in cli("windows") if w["pid"] in {p.pid for p in children}
                           and w["title"].startswith("RibbonWM QA Space Retention ") and w["onscreen"] and w["layer"] == 0]
                if len(windows) == 5:
                    break
                assert time.monotonic() < deadline
                time.sleep(.05)
            windows.sort(key=lambda w: w["pid"])
            managed, unmanaged = windows[:4], windows[4]
            with (ROOT / "docs/space-retention-daemon.txt").open("w") as log:
                daemon = subprocess.Popen([binary, "run", "--windows", ",".join(str(w["id"]) for w in managed),
                    "--config", str(config), "--exclude-app", "com.openai.*", "--exclude-app", "ChatGPT*",
                    "--exclude-app", "com.apple.systempreferences"], stdout=log, stderr=log)
                wait(lambda s: len(s["placements"]) == 4)
                time.sleep(.8)
                cli("focus-window", managed[-1]["id"])
                settled()
                cli("--monitor", monitor, "scroll", "-600")
                before = copy.deepcopy(layout(settled()))
                assert 100 < before["scroll"]["position"] < 1800
                widths = [c["width"] for c in before["columns"]]
                time.sleep(1.2)  # Several snapshots of partially clipped surfaces.
                assert [c["width"] for c in layout(cli("status"))["columns"]] == widths, "Clipping was adopted as a resize"
                print("PASS: partially clipped columns retain physical widths across inventory refreshes", flush=True)

                # Native activation changes logical focus, without a WM command.
                for w in (unmanaged, managed[0]):
                    subprocess.run([str(HELPER), "--focus-fixture", str(w["id"]), str(w["pid"])], check=True)
                    time.sleep(.3)
                after = layout(cli("status"))
                if "--spaces-only" not in sys.argv[3:]:
                    assert abs(after["scroll"]["position"] - before["scroll"]["position"]) < .1, (before, after)
                    assert after["columns"] == before["columns"]
                    print("PASS: return from an unmanaged owned app observes focus without resetting the viewport", flush=True)
                else:
                    print("SKIP: app activation retention assertion (isolating Space behavior)", flush=True)

                # Preserve a reordered strip and manual offset through real OS transitions.
                cli("focus-window", managed[1]["id"])
                cli("--monitor", monitor, "swap", "right")
                settled()
                cli("--monitor", monitor, "scroll", "200")
                baseline = copy.deepcopy(layout(settled()))
                for _ in range(3):
                    switch(direction, other)
                    away = layout(cli("status"))
                    assert away["columns"] == baseline["columns"], (baseline, away)
                    assert abs(away["scroll"]["position"] - baseline["scroll"]["position"]) < .1
                    switch(reverse, original_space)
                    returned = layout(settled())
                    assert returned["columns"] == baseline["columns"], (baseline, returned)
                    assert abs(returned["scroll"]["position"] - baseline["scroll"]["position"]) < .1, (baseline, returned)
                    assert returned["scroll"]["target"] == baseline["scroll"]["target"]
                print("PASS: three real Space round trips retain column order, widths and manual scroll offset", flush=True)

                # A real click must still reveal a partially visible focused column.
                cli("focus-window", managed[-1]["id"])
                settled()
                position = layout(cli("status"))["scroll"]["position"]
                cli("--monitor", monitor, "scroll", str(300 - position))
                settled()
                first = next(w for w in managed if w["id"] == layout(cli("status"))["columns"][0]["windows"][0])
                subprocess.run([str(HELPER), "--click-fixture", str(first["id"]), str(first["pid"])], check=True)
                wait(lambda s: s["native_focused_window"] == first["id"] and abs(layout(s)["scroll"]["target"] + 24) < 2)
                settled()
                print("PASS: an actual click still reveals the selected column", flush=True)
                assert {p["window"] for p in cli("status")["placements"]} == {w["id"] for w in managed}
        finally:
            if daemon is not None and daemon.poll() is None:
                cli("quit")
                daemon.wait(timeout=10)
            # Restore the original Space before closing the owned app windows.
            for _ in order:
                current = native_space()
                if current == original_space:
                    break
                step = "next" if order.index(current) < index else "previous"
                subprocess.run([str(HELPER), "--space", step], check=True)
                time.sleep(.8)
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)
    assert cli("backend-status")["controlled"] == 0
    assert native_space() == original_space
    print("PASS: owned fixtures/leases released; original Space restored", flush=True)


if __name__ == "__main__":
    main(*sys.argv[1:3])
