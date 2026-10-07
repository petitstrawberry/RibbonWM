"""Measure lifecycle and size stability using owned windows in the running WM.

Never launches, stops, or resets the user's daemon. No synthesized input or AX
queries into other applications. The fixture closes only its own windows.
"""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "native/build/test-window"
BINARY = sys.argv[1] if len(sys.argv) > 1 else ROOT / "target/debug/ribbonwm"


def cli(*args):
    return json.loads(subprocess.check_output([str(BINARY), *map(str, args)], timeout=4))


def main():
    assert os.environ.get("IN_NIX_SHELL")
    assert cli("status")["mode"] == "live"
    fixture = subprocess.Popen([str(HELPER), "--fixture", json.dumps(dict(
        regular=True, geometry_test=True, backdrops=[], lifetime=120,
        target=dict(x=300, y=150)))], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, text=True, bufsize=1)
    owned = set()

    def read():
        assert select.select([fixture.stdout], [], [], 5)[0], "Fixture timed out"
        return json.loads(fixture.stdout.readline())

    def command(op, **values):
        fixture.stdin.write(json.dumps(dict(op=op, **values)) + "\n")
        fixture.stdin.flush()
        return read()

    def wait(predicate):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            status = cli("status")
            assert not status["native_overview"], "Mission Control interrupted measurement"
            if predicate(status):
                return status
            time.sleep(.01)
        raise AssertionError("Live daemon did not converge")

    def exists(status, wid):
        return any(p["window"] == wid for p in status["placements"])

    try:
        ready = read()
        assert ready.get("ready"), ready
        wid, pid = ready["wid"], ready["pid"]
        owned.add(wid)
        command("present")
        wait(lambda s: exists(s, wid))
        creations, removals = [], []
        for _ in range(5):
            started = time.monotonic()
            child = command("new")["created"]
            owned.add(child)
            wait(lambda s: exists(s, child))
            creations.append(round((time.monotonic() - started) * 1000, 1))
            started = time.monotonic()
            command("close-new")
            wait(lambda s: not exists(s, child))
            removals.append(round((time.monotonic() - started) * 1000, 1))
        print(json.dumps(dict(create_ms=creations, remove_ms=removals)), flush=True)

        child = command("new")["created"]
        owned.add(child)
        status = wait(lambda s: exists(s, child))
        monitor = next(p["monitor"] for p in status["placements"] if p["window"] == wid)
        time.sleep(.4)
        requests_before = cli("status").get("native_size_requests")
        sampler = subprocess.Popen([str(HELPER), "--trace-window", str(wid), str(pid), "3"],
                                   stdout=subprocess.PIPE, text=True)
        assert json.loads(sampler.stdout.readline())["tracing"]
        # Drain concurrently so the sampler never waits on a full output pipe.
        import threading
        samples = []
        reader = threading.Thread(target=lambda: samples.extend(
            json.loads(line) for line in sampler.stdout), daemon=True)
        reader.start()
        for target in [wid, child, wid, child, wid]:
            cli("focus-window", target)
            time.sleep(.12)
        for delta in [300, -600, 300]:
            cli("--monitor", monitor, "scroll", delta)
            time.sleep(.12)
        sampler.wait(timeout=5)
        reader.join(timeout=1)
        assert sampler.returncode == 0 and len(samples) >= 60
        dimensions = {(s["frame"]["width"], s["frame"]["height"]) for s in samples}
        assert len(dimensions) == 1, dimensions
        assert all(s["errors"] == [0, 0] for s in samples)
        print(json.dumps(dict(focus_scroll_samples=len(samples), native_sizes=list(dimensions))), flush=True)
        if requests_before is not None:
            requests_after = cli("status")["native_size_requests"]
            assert requests_after == requests_before, (requests_before, requests_after)
            print("PASS: focus/scroll issued zero native size requests", flush=True)
    finally:
        if fixture.poll() is None:
            fixture.stdin.write('{"op":"quit"}\n')
            fixture.stdin.flush()
            fixture.wait(timeout=5)
        fixture.stdin.close()
        fixture.stdout.close()
    wait(lambda s: all(not exists(s, wid) for wid in owned))
    print("PASS: fixture windows removed; original live daemon still running", flush=True)


if __name__ == "__main__":
    main()
