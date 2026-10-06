"""Real border drag, using one disposable Alacritty."""
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
support = runpy.run_path(str(ROOT / "scripts/smoke-live.py"))
raw_cli, inspect = support["cli"], support["inspect"]
HELPER = ROOT / "native/build/test-window"


def cli(*args):
    for attempt in range(3):
        try:
            return raw_cli(*args)
        except RuntimeError as error:
            if args != ("status",) or attempt == 2 or "Resource temporarily unavailable" not in str(error):
                raise
            print("Read-only status reply deadline exceeded; retrying observation", flush=True)
            time.sleep(.2)


def main(alacritty):
    assert os.environ.get("IN_NIX_SHELL")
    assert cli("status")["mode"] == "live"
    displays = cli("displays")
    primary = next(d for d in displays if d["primary"])
    processes = []

    def wait(predicate, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.06)
        raise RuntimeError("Interaction did not reach the expected state")

    def selected(wid):
        value = cli("status")
        return value if value["native_focused_window"] == wid else None

    with tempfile.TemporaryDirectory(prefix="ribbon-interaction-") as temporary:
        def spawn(display, index):
            x = round((display["viewport"]["x"] + 100) * display["scale"])
            y = round((display["viewport"]["y"] + 100) * display["scale"])
            p = subprocess.Popen([alacritty, "--title", f"RibbonWM QA Interaction {index}",
                                  "-o", "window.dimensions.columns=120", "-o", "window.dimensions.lines=30",
                                  "-o", f"window.position.x={x}", "-o", f"window.position.y={y}",
                                  "-e", sys.executable, str(ROOT / "scripts/smoke-live.py"),
                                  "--terminal-child", str(index), str(Path(temporary) / f"input-{index}.json")],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            processes.append(p)
            def enrolled():
                managed = cli("status")["original_geometry"]
                return next((w for w in cli("windows") if w["pid"] == p.pid and w["onscreen"] and w["layer"] == 0
                             and w["title"].startswith("RibbonWM QA ") and str(w["id"]) in managed), None)
            window = wait(enrolled)
            wid = window["id"]
            print("Discovered interaction fixture:", window, flush=True)
            return wid, p

        try:
            a, pa = spawn(primary, 0)
            subprocess.run([str(HELPER), "--focus-fixture", str(a), str(pa.pid)], check=True, timeout=3)
            wait(lambda: selected(a))
            time.sleep(1)
            before = inspect(a)["frame"]["width"]
            subprocess.run([str(HELPER), "--drag-fixture", str(a), str(pa.pid), "110", "0"], check=True, timeout=3)
            wait(lambda: abs(inspect(a)["frame"]["width"] - before) >= 40)
            def adopted():
                state = cli("status")
                plan = next(p for p in state["placements"] if p["window"] == a)
                native = inspect(a)
                return (plan, native) if abs(plan["frame"]["width"]-native["frame"]["width"]) < 2 else None
            plan, native = wait(adopted)
            time.sleep(.8)
            assert abs(inspect(a)["frame"]["width"]-native["frame"]["width"]) < 2
            print("PASS: real mouse border drag changes the owner width and the WM adopts it without snapping back:",
                  dict(before=before, after=native["frame"]["width"]), flush=True)
            start_x = -inspect(a)["transform"][4]
            result = subprocess.check_output([str(HELPER), "--move-drag-fixture", str(a), str(pa.pid), "110", "0"],
                                             text=True, timeout=3)
            samples = json.loads(result.splitlines()[0])["drag_samples"]
            positions = [-sample["transform"][4] for sample in samples]
            assert positions[-1] - start_x >= 40, (start_x, positions)
            assert all(b >= a-2 for a, b in zip(positions, positions[1:])), positions
            assert max(positions)-min(positions) >= 40, positions
            print("PASS: actual title-bar drag advances throughout the hold without WM snapping it back:", positions, flush=True)
            wait(lambda: abs(-inspect(a)["transform"][4] - next(
                p["frame"]["x"] for p in cli("status")["placements"] if p["window"] == a)) < 2)
            queried = cli("-m", "query", "--displays")
            assert next(d for d in queried if d["uuid"] == primary["id"])["has-focus"]
            print("PASS: display query matches native focus", flush=True)
        finally:
            for p in processes:
                if p.poll() is None:
                    p.terminate()
                    p.wait(timeout=5)
        wait(lambda: str(a) not in cli("status")["original_geometry"])
        print("PASS: interaction fixture closed; live WM remains running", flush=True)


if __name__ == "__main__":
    main(sys.argv[1])
