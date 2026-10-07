"""AX restore regression against an owned, translated AppKit window; no Dock needed."""
import json
import os
from pathlib import Path
import select
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "native/build/test-window"


def main():
    assert os.environ.get("IN_NIX_SHELL"), "Run via nix develop"
    fixture = subprocess.Popen([str(HELPER), "--fixture", json.dumps(dict(
        geometry_test=True, target=dict(x=300, y=150), backdrops=[], lifetime=30))],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def read():
        assert select.select([fixture.stdout], [], [], 3)[0], "Fixture timed out"
        line = fixture.stdout.readline()
        assert line, "Fixture exited"
        return json.loads(line)

    def command(op, **kwargs):
        fixture.stdin.write(json.dumps(dict(op=op, **kwargs)) + "\n")
        fixture.stdin.flush()
        return read()

    try:
        ready = read()
        assert ready.get("ready"), ready
        wid, pid = str(ready["wid"]), str(ready["pid"])
        assert command("present")["presented"]

        def geometry():
            result = subprocess.run([str(HELPER), "--geometry-fixture", wid, pid], capture_output=True, text=True)
            assert result.returncode == 0, result.stdout + result.stderr
            value = json.loads(result.stdout)
            assert value["geometry_error"] == 0, value
            return value["geometry"]

        # The live service may enroll even this accessory fixture. Exempt only
        # this disposable ID before independent AX/compositor tests.
        live = os.environ.get("RIBBONWM_QA_LIVE_BINARY")
        if live:
            deadline = time.monotonic()+8
            while time.monotonic()<deadline:
                result = subprocess.run([live, "float", "on", "--window", wid], capture_output=True, text=True)
                if result.returncode == 0 and json.loads(result.stdout).get("ok"):
                    break
                time.sleep(.05)
            else:
                raise AssertionError(result.stdout+result.stderr)
            time.sleep(.15)
            subprocess.run([str(HELPER), "--restore-fixture", wid, pid, "300", "150", "400", "422"], check=True)
        original = geometry()
        original_transform = command("state")["transform"]
        # Simulate a compositor offset: the AX owner's logical position stays
        # independent of the visible translation, as with Chrome titlebars.
        assert command("translate", x=520, y=250)["transform_error"] == 0
        assert geometry() == original, (geometry(), original)
        subprocess.run([str(HELPER), "--resize-fixture", wid, pid, "600", "550"], check=True)
        for _ in range(3):
            subprocess.run([str(HELPER), "--restore-fixture", wid, pid,
                *[str(original[k]) for k in ("x", "y", "width", "height")]], check=True)
            actual = geometry()
            assert all(abs(actual[k] - original[k]) <= 2 for k in original), (actual, original)
            assert actual["y"] >= 33, actual
        # Native anchoring can leave AppKit's cached frame at the old origin.
        # Reuse the pre-control calibration rather than interpreting that offset
        # as a decoration inset during the next resize.
        native = command("state")["frame"]
        calibration = [original[k] for k in ("x", "y", "width", "height")] + [native[k] for k in ("x", "y", "width", "height")]
        for x, y, width in [(520, 250, 600), (220, 170, 500), (650, 190, 700)]:
            assert command("translate", x=x, y=y)["transform_error"] == 0
            assert command("anchor-now")["error"] == 0
            subprocess.run([str(HELPER), "--resize-calibrated-fixture", wid, pid, str(width), "550", json.dumps(calibration)], check=True)
            state = command("state")
            assert abs(state["frame"]["x"]-x)<=2 and abs(state["frame"]["y"]-y)<=2, state
            assert abs(state["frame"]["width"]-width)<=2, state
        print("PASS: repeated native anchoring and calibrated AX resizing preserve the intended origin", flush=True)
        assert command("translate", x=-original_transform[4], y=-original_transform[5])["transform_error"] == 0
        print("PASS: AX snapshot remains logical while presentation is translated", flush=True)
        print("PASS: resize and repeated restoration keep the original top below the menu bar", flush=True)
    finally:
        if fixture.poll() is None:
            fixture.stdin.write('{"op":"quit"}\n')
            fixture.stdin.flush()
            fixture.wait(timeout=3)
        fixture.stdin.close()
        fixture.stdout.close()
    print("PASS: owned fixture closed", flush=True)


if __name__ == "__main__":
    main()
