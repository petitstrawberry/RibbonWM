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
    def session_active():
        return json.loads(subprocess.check_output([str(HELPER), "--session-state"], timeout=3))["session_active"]

    assert session_active(), "Unlock the Mac before running live GUI verification"
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
            assert session_active(), "Session locked during measurement"
            status = cli("status")
            assert not status["native_overview"], "Mission Control interrupted measurement"
            if predicate(status):
                return status
            time.sleep(.01)
        raise AssertionError("Live daemon did not converge")

    def exists(status, wid):
        return any(p["window"] == wid for p in status["placements"])

    def presented(status, wid):
        # Old releases expose no presentation acknowledgement; their historical
        # timings measure enrollment only and must remain labelled accordingly.
        return (wid in status["presented_windows"] if "presented_windows" in status
                else exists(status, wid))

    wid = None
    try:
        ready = read()
        assert ready.get("ready"), ready
        wid, pid = ready["wid"], ready["pid"]
        owned.add(wid)
        command("present")
        wait(lambda s: presented(s, wid))
        creations, removals, creation_commands, enrollments = [], [], [], []
        for _ in range(int(os.environ.get("RIBBONWM_QA_CYCLES", "20"))):
            started = time.monotonic()
            child = command("new")["created"]
            created = time.monotonic()
            owned.add(child)
            wait(lambda s: presented(s, child))
            creations.append(round((time.monotonic() - started) * 1000, 1))
            creation_commands.append(round((created - started) * 1000, 1))
            enrollments.append(round((time.monotonic() - created) * 1000, 1))
            started = time.monotonic()
            command("close-new")
            wait(lambda s: not exists(s, child))
            removals.append(round((time.monotonic() - started) * 1000, 1))
        print(json.dumps(dict(completion="presented" if "presented_windows" in cli("status") else "enrolled", create_ms=creations, remove_ms=removals,
                              app_creation_ms=creation_commands, after_creation_ms=enrollments)), flush=True)

        child = command("new")["created"]
        owned.add(child)
        status = wait(lambda s: presented(s, child))
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
    except Exception:
        if fixture.poll() is None and wid is not None and session_active():
            probe = subprocess.run([str(HELPER), "--geometry-fixture", str(wid), str(fixture.pid)],
                                   capture_output=True, text=True, timeout=3)
            print("Fixture AX diagnostic:", probe.stdout, probe.stderr, flush=True)
            print(json.dumps(dict(fixture=command("state"),
                                  windows=[w for w in cli("windows") if w["pid"] == fixture.pid],
                                  daemon=cli("status"))), flush=True)
        raise
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
