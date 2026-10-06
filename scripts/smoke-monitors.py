"""Probe a real physical display seam, including shadows and mouse input.

Only disposable, independently owned fixture panels are modified. The fixture
never sets WindowServer clips/transforms; the loaded Dock payload does that.
Run: nix develop -c python3 scripts/smoke-monitors.py
"""
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
INSPECT = ROOT / "native/build/test-window"
SESSION = f"monitor-smoke-{os.getpid()}-{time.time_ns()}"


def request(op, **values):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(0.5)
        stream.connect(f"/tmp/ribbonwm-{os.getuid()}/backend.sock")
        stream.sendall(json.dumps(dict(op=op, session=SESSION, **values)).encode() + b"\n")
        with stream.makefile("rb") as reader:
            result = json.loads(reader.readline(65536))
        assert result.get("ok"), result
        return result


def main():
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run via nix develop")
    subprocess.run(["cargo", "build", "--locked"], cwd=ROOT, check=True)
    subprocess.run(["make", "-C", str(ROOT / "native"), "build/test-window"], check=True)
    displays = json.loads(subprocess.check_output([str(ROOT / "target/debug/ribbonwm"), "displays"], text=True))
    pairs = [(a, b) for a in displays for b in displays
             if a["id"] != b["id"] and a["frame"]["width"] >= 650
             and b["frame"]["width"] >= 500
             and abs(a["frame"]["x"] + a["frame"]["width"] - b["frame"]["x"]) < 1
             and min(a["viewport"]["y"] + a["viewport"]["height"], b["viewport"]["y"] + b["viewport"]["height"])
             - max(a["viewport"]["y"], b["viewport"]["y"]) >= 650]
    if not pairs:
        raise RuntimeError("This probe requires horizontally adjacent displays with 650pt vertical overlap")
    left, right = pairs[0]
    seam = right["frame"]["x"]
    top = max(left["viewport"]["y"], right["viewport"]["y"]) + 180
    print("Physical seam:", json.dumps(dict(left=left, right=right, x=seam)), flush=True)
    status = request("hello")
    assert status["controlled"] == 0, "Finish the existing controller first"
    configuration = dict(shadow=True, target=dict(x=seam-500, y=top), backdrops=[
        dict(x=seam-600, y=top-100, width=600, height=600, surface="source"),
        dict(x=seam, y=top-100, width=500, height=600, surface="neighbor")])
    fixture = subprocess.Popen([str(INSPECT), "--fixture", json.dumps(configuration)],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    events = queue.Queue()

    def read_events():
        for line in fixture.stdout:
            events.put(json.loads(line))

    threading.Thread(target=read_events, daemon=True).start()

    def command(op, **values):
        fixture.stdin.write(json.dumps(dict(op=op, **values)) + "\n")
        fixture.stdin.flush()

    def event():
        value = events.get(timeout=2)
        print("Fixture:", value, flush=True)
        return value

    def state():
        command("state")
        value = event()
        assert value.get("event") == "state" and value["errors"] == [0, 0]
        return value

    def click(x, surface, local_x, local_y):
        command("click", x=x, y=top+200)
        value = event()
        assert value.get("event") == "click" and value["surface"] == surface, value
        assert abs(value["x"]-local_x) < 2 and abs(value["y"]-local_y) < 2, value

    def screenshot(name, region):
        path = ROOT / f"docs/monitor-{name}.png"
        subprocess.run(["/usr/sbin/screencapture", "-x", "-R" + ",".join(str(int(n)) for n in region), str(path)], check=True)
        return path

    neighbor_region = (seam, top-60, 350, 520)
    try:
        ready = event()
        assert ready.get("ready") and ready["pid"] not in (os.getpid(), status["pid"])
        wid = ready["wid"]
        command("present")
        assert event().get("presented")
        time.sleep(0.2)
        original = state()
        baseline = screenshot("neighbor-baseline", neighbor_region)
        click(seam+50, "neighbor", 50, 300)

        def apply(x, width):
            clip = dict(x=x, y=top, width=width, height=400) if width else None
            assert request("frame", updates=[dict(wid=wid, frame=dict(x=x, y=top, width=400, height=400), clip=clip)])["controlled"] == 1

        # Negative control: a transformed window with its full content clip.
        apply(seam-150, 400)
        time.sleep(0.15)
        unbounded = screenshot("neighbor-unbounded", neighbor_region)
        command("click", x=seam+50, y=top+200)
        print("Unbounded neighbor input:", event(), flush=True)
        apply(seam-150, 150)
        clipped = state()
        assert clipped["transform"] == [1, 0, 0, 1, -(seam-150), -top]
        assert clipped["clip_bounds"] == [0, 0, 150, 400]
        time.sleep(0.15)
        bounded = screenshot("neighbor-clipped", neighbor_region)
        screenshot("seam-clipped", (seam-300, top-60, 650, 520))
        click(seam-50, "target", 100, 200)
        apply(seam-150, 150)
        click(seam+50, "neighbor", 50, 300)
        print("PASS: source input is transformed; adjacent physical monitor input passes through", flush=True)
        # Traverse the real seam, including a completely off-viewport window.
        for offset in (-350, -200, -50, 0, 50, -150):
            x = seam + offset
            apply(x, min(400, max(0, seam-x)))
            click(seam+100, "neighbor", 100, 300)
        print("PASS: crossing and fully hidden positions do not intercept neighbor clicks", flush=True)
        request("reset")
        restored = state()
        assert restored["transform"] == original["transform"] and restored["clip_bounds"] == original["clip_bounds"]
        click(seam-400, "target", 100, 200)
        print("PASS: original owner geometry, clip and input restored", flush=True)
    finally:
        try:
            request("reset")
        finally:
            if fixture.poll() is None:
                command("quit")
                try:
                    fixture.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    fixture.terminate()
                    fixture.wait(timeout=2)
            fixture.stdin.close()
            fixture.stdout.close()
    assert request("hello")["controlled"] == 0
    compare = lambda a, b: json.loads(subprocess.check_output([str(INSPECT), "--compare-images", str(a), str(b)], text=True))
    negative = compare(baseline, unbounded)
    positive = compare(baseline, bounded)
    print("Neighbor pixels, full window:", negative, flush=True)
    print("Neighbor pixels, clipped shadowed window:", positive, flush=True)
    assert positive["changed"] == 0, "Drawing or shadow leaked into the adjacent display"
    print("PASS: adjacent-display screenshot matches baseline pixel-for-pixel within 2/255; no fixture or controlled windows remain", flush=True)


if __name__ == "__main__":
    main()
