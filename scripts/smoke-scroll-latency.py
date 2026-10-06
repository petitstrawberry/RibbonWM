"""Measure command-to-transform latency using four explicitly owned windows.

Run via nix develop. An existing production WM must be stopped first.
Reports compositor updates, not physical display refresh rate.
"""
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "native/build/test-window"


def main():
    assert os.environ.get("IN_NIX_SHELL")
    binary, alacritty, duration = sys.argv[1:4]

    def cli(*args):
        return json.loads(subprocess.check_output([binary, *map(str, args)], timeout=5))

    assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    assert cli("backend-status")["controlled"] == 0
    children, daemon = [], None
    with tempfile.TemporaryDirectory(prefix="ribbon-latency-") as temporary:
        config = Path(temporary) / "config.toml"
        config.write_text("padding_top=24.0\npadding_bottom=24.0\npadding_left=24.0\npadding_right=24.0\n"
                          "gap=6.0\npreserve_window_width=true\ncenter_content=true\nframe_rate=120\n"
                          f"animation_curve='{sys.argv[4] if len(sys.argv)>4 else 'ease_in_out'}'\nanimation_duration={float(duration)}\n")
        try:
            for i in range(4):
                children.append(subprocess.Popen([alacritty, "--title", f"RibbonWM QA Latency {i}",
                    "-o", "window.dimensions.columns=120", "-o", "window.dimensions.lines=25",
                    "-e", sys.executable, "-c", "import time; time.sleep(90)"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
                time.sleep(.15)
            deadline = time.monotonic() + 8
            while True:
                windows = [w for w in cli("windows") if w["pid"] in [p.pid for p in children]
                           and w["onscreen"] and w["layer"] == 0 and w["title"].startswith("RibbonWM QA Latency ")]
                if len(windows) == 4:
                    break
                assert time.monotonic() < deadline
                time.sleep(.05)
            windows.sort(key=lambda w: w["id"])
            with open(ROOT / "docs/latency-daemon.txt", "w") as log:
                daemon = subprocess.Popen([binary, "run", "--windows", ",".join(str(w["id"]) for w in windows),
                    "--config", str(config), "--exclude-app", "com.openai.*", "--exclude-app", "ChatGPT*",
                    "--exclude-app", "com.apple.systempreferences"], stdout=log, stderr=log)

            def settled():
                deadline = time.monotonic() + 10
                while True:
                    assert daemon.poll() is None
                    try:
                        status = cli("status")
                        if len(status["placements"]) == 4:
                            for m in status["state"]["monitors"].values():
                                s = m["contexts"][str(m["native_space"])]["scroll"]
                                if abs(s["target"] - s["position"]) > .01:
                                    break
                            else:
                                return
                    except subprocess.CalledProcessError:
                        pass
                    assert time.monotonic() < deadline
                    time.sleep(.02)

            settled()
            deadline = time.monotonic() + 10
            while True:
                plans = cli("status")["placements"]
                ready = True
                for plan in plans:
                    native = json.loads(subprocess.check_output([str(HELPER), str(plan["window"])]))
                    if any(abs(native["frame"][k] - plan["frame"][k]) > 2 for k in ("width", "height")):
                        ready = False
                        break
                if ready:
                    break
                assert time.monotonic() < deadline, "Initial AX sizing did not finish"
                time.sleep(.05)
            cli("focus-window", windows[0]["id"])
            settled()
            measurements = []
            for target in [windows[-1], windows[0]] * 3:
                sampler = subprocess.Popen([str(HELPER), "--sample-surface", str(windows[0]["id"]),
                    str(windows[0]["pid"]), ".7"], stdout=subprocess.PIPE, text=True)
                assert json.loads(sampler.stdout.readline())["sampling"]
                time.sleep(.02)
                issued = time.monotonic()
                cli("focus-window", target["id"])
                reply = time.monotonic()
                samples = json.loads(sampler.stdout.readline())["samples"]
                sampler.wait(timeout=2)
                assert sampler.returncode == 0
                changes = []
                previous = samples[0]["transform"][4]
                for sample in samples[1:]:
                    x = sample["transform"][4]
                    if abs(x - previous) > .01:
                        changes.append(sample["time"])
                        previous = x
                assert len(changes) >= 2, "Animation stalled or was not observed"
                gaps = [b - a for a, b in zip(changes, changes[1:])]
                measurements.append(dict(first_ms=(changes[0]-issued)*1000,
                    total_ms=(changes[-1]-issued)*1000, reply_ms=(reply-issued)*1000,
                    updates=len(changes), mean_hz=(len(changes)-1)/(changes[-1]-changes[0]) if gaps else 0,
                    max_gap_ms=max(gaps)*1000 if gaps else 0))
                settled()
            print(json.dumps(dict(binary=binary, duration=float(duration), measurements=measurements,
                median_first_ms=statistics.median(m["first_ms"] for m in measurements),
                median_total_ms=statistics.median(m["total_ms"] for m in measurements),
                median_hz=statistics.median(m["mean_hz"] for m in measurements)), indent=2), flush=True)
        finally:
            if daemon and daemon.poll() is None:
                cli("quit")
                daemon.wait(timeout=8)
            for child in children:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=3)
    assert cli("backend-status")["controlled"] == 0


if __name__ == "__main__":
    main()
