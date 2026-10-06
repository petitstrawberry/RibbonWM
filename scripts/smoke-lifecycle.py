"""Close one independent app window while a second holds its Dock placement.

Run inside nix develop after loading protocol 2. Only fixture windows are changed.
"""
import json
import os
import queue
import runpy
from pathlib import Path
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
request = runpy.run_path(str(ROOT / "scripts/smoke-backend.py"))["request"]


def main():
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run: nix develop -c python3 scripts/smoke-lifecycle.py")
    status = request("hello")
    assert status["version"] == 2 and status["controlled"] == 0, status
    subprocess.run(["make", "-C", str(ROOT / "native"), "build/test-window"], check=True)
    fixtures = []
    try:
        updates = []
        for x in (800, 1400):
            p = subprocess.Popen([str(ROOT / "native/build/test-window"), "--fixture",
                                  json.dumps(dict(backdrops=[], target=dict(x=x, y=360)))],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
            events = queue.Queue()
            fixtures.append((p, events))
            def read(process=p, output=events):
                for line in process.stdout:
                    output.put(json.loads(line))
            threading.Thread(target=read, daemon=True).start()
            ready = events.get(timeout=3)
            assert ready.get("ready") and ready["pid"] == p.pid
            p.stdin.write('{"op":"present"}\n')
            p.stdin.flush()
            assert events.get(timeout=2).get("presented")
            updates.append(dict(wid=ready["wid"], pid=p.pid,
                                frame=dict(x=x+100, y=360, width=400, height=400),
                                clip=dict(x=x+100, y=360, width=200, height=400)))
        time.sleep(0.15)
        assert request("frame", updates=updates)["controlled"] == 2

        def state(index):
            p, events = fixtures[index]
            p.stdin.write('{"op":"state"}\n')
            p.stdin.flush()
            value = events.get(timeout=2)
            assert value["errors"] == [0, 0] and value["clip_error"] == 0, value
            return value

        survivor = state(1)
        p, _ = fixtures[0]
        p.stdin.write('{"op":"quit"}\n')
        p.stdin.flush()
        p.wait(timeout=3)
        assert request("frame", updates=updates)["controlled"] == 1
        after = state(1)
        assert after["transform"] == survivor["transform"]
        assert after["clip_bounds"] == survivor["clip_bounds"]
        assert request("frame", updates=updates[1:])["controlled"] == 1
        print("PASS: a closed/repeated stale ID does not reject frames or reset a surviving app's transform/clip")
    finally:
        try:
            request("reset")
        finally:
            for p, _ in fixtures:
                if p.poll() is None:
                    p.stdin.write('{"op":"quit"}\n')
                    p.stdin.flush()
                    try:
                        p.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        p.terminate()
                        p.wait(timeout=3)
                p.stdin.close()
                p.stdout.close()
    assert request("hello")["controlled"] == 0
    print("PASS: fixture processes and controlled windows cleaned up")


if __name__ == "__main__":
    main()
