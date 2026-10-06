"""Owned app only: notification latency, manual resize and menu-bar invariants."""
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/debug/ribbonwm"
HELPER = ROOT / "native/build/test-window"


def cli(*args):
    return json.loads(subprocess.check_output([str(BINARY), *map(str, args)], timeout=3))


def main():
    assert os.environ.get("IN_NIX_SHELL")
    assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    fixture = subprocess.Popen([str(HELPER), "--fixture", json.dumps(dict(
        regular=True, geometry_test=True, target=dict(x=300, y=150), backdrops=[], lifetime=30))],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    daemon = None

    def read():
        assert select.select([fixture.stdout], [], [], 3)[0], "Fixture timed out"
        return json.loads(fixture.stdout.readline())

    def command(op, **values):
        fixture.stdin.write(json.dumps(dict(op=op, **values)) + "\n")
        fixture.stdin.flush()
        return read()

    def wait(predicate, timeout=3):
        deadline = time.monotonic() + timeout
        while True:
            assert daemon.poll() is None, "WM exited; see docs/events-daemon.txt"
            try:
                status = cli("status")
                if predicate(status):
                    return status
            except subprocess.CalledProcessError:
                pass
            assert time.monotonic() < deadline, "WM did not converge"
            time.sleep(.01)

    try:
        ready = read()
        assert ready.get("ready"), ready
        wid, pid = ready["wid"], ready["pid"]
        assert command("present")["presented"]
        apps = cli("applications")
        assert any(a["pid"] == pid for a in apps)
        own = next(a for a in apps if a["pid"] == pid)
        # Exclude every other regular app before AX observer registration.
        excluded = {"com.openai.*", "ChatGPT*"}
        for app in apps:
            if app["pid"] != pid:
                excluded.add(app["bundle_id"] or app["app"])
        assert own["app"] not in excluded and own["bundle_id"] not in excluded
        with tempfile.TemporaryDirectory(prefix="ribbon-events-") as temporary:
            config = Path(temporary) / "config.toml"
            config.write_text("horizontal_margin=24.0\nvertical_margin=24.0\ngap=6.0\n"
                              "preserve_window_width=true\ncenter_content=true\nframe_rate=120\n")
            with open(ROOT / "docs/events-daemon.txt", "w") as log:
                args = [str(BINARY), "run", "--all", "--config", str(config)]
                for app in sorted(excluded):
                    args += ["--exclude-app", app]
                daemon = subprocess.Popen(args, stdout=log, stderr=log,
                                          env=dict(os.environ, RIBBONWM_EVENT_TRACE="1"))
                initial = wait(lambda s: any(p["window"] == wid for p in s["placements"]))
                height = next(p["frame"]["height"] for p in initial["placements"] if p["window"] == wid)
                time.sleep(.2)
                start = time.monotonic()
                assert command("resize", width=700, height=height)["resized"]
                wait(lambda s: any(p["window"] == wid and abs(p["frame"]["width"]-700)<2 for p in s["placements"]))
                resize_latency = time.monotonic() - start
                start = time.monotonic()
                new = command("new")["created"]
                status = wait(lambda s: any(p["window"] == new for p in s["placements"]))
                creation_latency = time.monotonic() - start
                assert creation_latency < .4, creation_latency
                assert resize_latency < .4, resize_latency
                assert {p["window"] for p in status["placements"]} == {wid, new}
                time.sleep(.25)
                for placement in cli("status")["placements"]:
                    native = json.loads(subprocess.check_output([str(HELPER), str(placement["window"])]))
                    viewport = cli("status")["state"]["monitors"][placement["monitor"]]["viewport"]
                    assert -native["transform"][5] >= viewport["y"] + 24 - 2, native
                    assert native["clip_bounds"][3] > 0, native
                print(f"PASS: new-window enrollment {creation_latency:.3f}s; manual width adoption {resize_latency:.3f}s", flush=True)
                print("PASS: only owned windows managed; rendered tops stay below the menu bar plus padding", flush=True)
                assert command("refuse-width", width=500)["refusing"] == 500
                assert command("close-new").get("closed_new")
                wait(lambda s: len(s["placements"]) == 1)
                cli("resize", "500")
                failed = wait(lambda s: s["window_modes"][str(wid)]["floating"])
                assert not failed["placements"]
                assert daemon.poll() is None
                assert command("new").get("created")
                wait(lambda s: len(s["placements"]) == 1)
                print("PASS: refusing owner becomes floating; WM survives and enrolls the next new window", flush=True)
                cli("quit")
                daemon.wait(timeout=5)
                assert daemon.returncode == 0
            flags = [int(line.split(": ")[1]) for line in (ROOT / "docs/events-daemon.txt").read_text().splitlines()
                     if line.startswith("Native events: ")]
            assert any(f & 4 for f in flags), "No AX geometry notifications received"
            assert any(f & 1 for f in flags), "No window lifecycle notifications received"
            print("PASS: AX geometry and window lifecycle notifications received", flush=True)
    finally:
        if daemon is not None and daemon.poll() is None:
            try:
                cli("quit")
                daemon.wait(timeout=5)
            except Exception:
                daemon.terminate()
                daemon.wait(timeout=3)
        if fixture.poll() is None:
            fixture.stdin.write('{"op":"quit"}\n')
            fixture.stdin.flush()
            fixture.wait(timeout=3)
        fixture.stdin.close()
        fixture.stdout.close()
    assert cli("backend-status")["controlled"] == 0
    print("PASS: fixtures, managed geometry and backend leases released", flush=True)


if __name__ == "__main__":
    main()
