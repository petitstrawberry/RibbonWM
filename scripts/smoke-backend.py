"""Real cross-process Dock transform, clip, hit-testing and restoration probe.

Run through `nix develop -c python3 scripts/smoke-backend.py` after loading
the Dock payload. The AppKit fixture never calls a compositor setter.
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
SESSION = f"backend-smoke-{os.getpid()}-{time.time_ns()}"


def request(op, **values):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(0.5)
        stream.connect(f"/tmp/ribbonwm-{os.getuid()}/backend.sock")
        stream.sendall(json.dumps(dict(op=op, session=SESSION, **values)).encode() + b"\n")
        with stream.makefile("rb") as reader:
            result = json.loads(reader.readline(65536))
        if not result.get("ok"):
            raise RuntimeError(result)
        return result


def main():
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run via nix develop -c python3 scripts/smoke-backend.py")
    subprocess.run(["make", "-C", str(ROOT / "native"), "build/test-window"], check=True)
    status = request("hello")
    assert status["controlled"] == 0, "Another session is already controlling windows"
    print("Dock status:", status, flush=True)
    fixture = subprocess.Popen([str(ROOT / "native/build/test-window")], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True, bufsize=1)
    events = queue.Queue()

    def read_events():
        for line in fixture.stdout:
            events.put(json.loads(line))

    threading.Thread(target=read_events, daemon=True).start()

    def command(op, **values):
        fixture.stdin.write(json.dumps(dict(op=op, **values)) + "\n")
        fixture.stdin.flush()

    def event():
        value = events.get(timeout=1.5)
        print("Fixture:", value, flush=True)
        return value

    def state():
        command("state")
        value = event()
        assert value["event"] == "state" and value["errors"] == [0, 0]
        return value

    def click(x, target, local_x):
        command("click", x=x, y=560)
        value = event()
        assert value["event"] == "click" and value["target"] == target, value
        assert abs(value["x"] - local_x) < 2 and abs(value["y"] - (200 if target else 240)) < 2, value
        # The reported event is mouse-down. Await its scheduled mouse-up before
        # changing a transform/clip or moving to the next hit-testing probe.
        time.sleep(0.1)

    try:
        ready = event()
        assert ready.get("ready") and ready["pid"] != os.getpid() and ready["pid"] != status["pid"]
        # Save a normally visible app state, rather than the fixture's alpha-zero
        # staging clip. AppKit changes its clip when an invisible panel is revealed.
        command("present")
        assert event().get("presented")
        time.sleep(0.15)
        original = state()
        wid = ready["wid"]
        frame = dict(x=820, y=360, width=400, height=400)
        clip = dict(x=900, y=360, width=240, height=400)

        def apply(visible):
            return request("frame", updates=[dict(wid=wid, frame=frame, clip=clip if visible else None)])

        assert apply(True)["controlled"] == 1
        transformed = state()
        assert transformed["transform"] == [1, 0, 0, 1, -820, -360]
        time.sleep(0.15)
        subprocess.run(["/usr/sbin/screencapture", "-x", "-R780,320,600,480",
                        str(ROOT / "docs/backend-clipped.png")], check=True)
        click(920, True, 100)
        print("PASS: Dock controls a different process; transformed input reaches local (100, 200)", flush=True)
        apply(True)
        click(850, False, 70)
        print("PASS: input outside clip passes through to the backdrop", flush=True)
        apply(False)
        click(920, False, 140)
        print("PASS: empty clip removes the external window from mouse hit-testing", flush=True)
        subprocess.run(["/usr/sbin/screencapture", "-x", "-R780,320,600,480",
                        str(ROOT / "docs/backend-hidden.png")], check=True)
        apply(True)
        state()
        click(920, True, 100)
        print("PASS: nonempty clip reveals the hidden window again", flush=True)
        request("reset")
        assert state()["transform"] == original["transform"]
        time.sleep(0.15)
        subprocess.run(["/usr/sbin/screencapture", "-x", "-R780,320,600,480",
                        str(ROOT / "docs/backend-restored.png")], check=True)
        click(1040, True, 100)
        print("PASS: explicit reset restores transform, drawing and input", flush=True)
        apply(False)
        time.sleep(2.5)
        assert request("hello")["controlled"] == 0
        assert state()["transform"] == original["transform"]
        click(1040, True, 100)
        print("PASS: heartbeat expiry restores the external window without a client reset", flush=True)
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
    print("PASS: no controlled windows or fixture process remain", flush=True)


if __name__ == "__main__":
    main()
