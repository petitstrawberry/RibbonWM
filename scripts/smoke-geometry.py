"""AX restore regression against an owned, translated AppKit window; no Dock needed."""
import json
import os
from pathlib import Path
import select
import subprocess

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

        original = geometry()
        original_transform = command("state")["transform"]
        # Simulate a compositor offset: the AX owner's logical position stays
        # independent of the visible translation, as with Chrome titlebars.
        assert command("translate", x=520, y=250)["transform_error"] == 0
        assert geometry() == original, (geometry(), original)
        subprocess.run([str(HELPER), "--resize-fixture", wid, pid, "600", "550"], check=True, stdout=subprocess.DEVNULL)
        for _ in range(3):
            subprocess.run([str(HELPER), "--restore-fixture", wid, pid,
                *[str(original[k]) for k in ("x", "y", "width", "height")]], check=True, stdout=subprocess.DEVNULL)
            actual = geometry()
            assert all(abs(actual[k] - original[k]) <= 2 for k in original), (actual, original)
            assert actual["y"] >= 33, actual
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
