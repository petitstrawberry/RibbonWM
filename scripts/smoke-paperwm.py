"""PaperWM rules against disposable real apps in an already running live WM.

The existing desktop remains managed by the user's live session. This probe
activates/closes only the Alacritty processes it creates, then leaves that WM
running. It never types into a user's shell or controls excluded applications.
"""
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
cli, inspect = support["cli"], support["inspect"]
HELPER = ROOT / "native/build/test-window"


def main(alacritty):
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run inside nix develop")
    first = cli("status")
    assert first["mode"] == "live"
    settings = first["state"]["settings"]
    assert settings["preserve_window_width"] and settings["center_content"]
    processes = []
    owned = {}
    monitor_id = None
    # Pin fixture startup to the primary screen. AppKit's default placement
    # follows desktop focus and can change after the mouse-focus probe.
    display = next(d for d in cli("displays") if d["primary"])
    startup_x = round((display["viewport"]["x"] + 100) * display["scale"])
    startup_y = round((display["viewport"]["y"] + 100) * display["scale"])

    def snapshot():
        # AX work is synchronous; the live desktop can briefly exceed the
        # two-second IPC reply deadline. Retry only this read-only observation,
        # never a focus/scroll/resize command with a possibly committed effect.
        for attempt in range(3):
            try:
                value = cli("status")["state"]
                break
            except RuntimeError as error:
                if attempt == 2 or "Resource temporarily unavailable" not in str(error):
                    raise
                print("Read-only status reply deadline exceeded; retrying observation", flush=True)
                time.sleep(0.1)
        managed = {wid for m in value["monitors"].values() for s in m["contexts"].values()
                   for c in s["columns"] for wid in c["windows"]}
        inventory = cli("windows")
        assert not any(w["id"] in managed and w["bundle_id"].startswith("com.openai.") for w in inventory)
        return value

    def context():
        m = snapshot()["monitors"][monitor_id]
        return m, m["contexts"][str(m["native_space"])]

    def wait(predicate, reason, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(0.06)
        raise RuntimeError(reason)

    def settle():
        def stable():
            m, row = context()
            scroll = row["scroll"]
            return (m, row) if abs(scroll["position"]-scroll["target"]) < 0.01 and abs(scroll["velocity"]) < 0.01 else None
        return wait(stable, "Paper scroll did not settle")

    def order(row):
        return [wid for c in row["columns"] for wid in c["windows"]]

    def selected(row):
        return row["columns"][row["focused_column"]]["windows"][row["focused_row"]]

    with tempfile.TemporaryDirectory(prefix="ribbon-paper-") as temporary:
        def padding(edge, legacy):
            value = settings.get(f"padding_{edge}")
            return settings[legacy] if value is None else value

        def spawn(index, columns):
            nonlocal monitor_id
            p = subprocess.Popen([alacritty, "--title", f"RibbonWM QA Paper {index}",
                                  "-o", f"window.position.x={startup_x}", "-o", f"window.position.y={startup_y}",
                                  "-o", "window.opacity=1.0", "-o", f"window.dimensions.columns={columns}",
                                  "-o", "window.dimensions.lines=30", "-e", sys.executable,
                                  str(ROOT / "scripts/smoke-live.py"), "--terminal-child", str(index),
                                  str(Path(temporary) / f"input-{index}.json")],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            processes.append(p)
            def discover():
                if p.poll() is not None:
                    raise RuntimeError("Disposable Alacritty exited during discovery")
                return next((w for w in cli("windows") if w["pid"] == p.pid and w["onscreen"]
                             and w["layer"] == 0 and w["bounds"]["width"] > 100), None)
            window = wait(discover, "New Alacritty did not appear")
            wid = window["id"]
            owned[wid] = p
            def enrolled():
                for mid, m in snapshot()["monitors"].items():
                    row = m["contexts"][str(m["native_space"])]
                    if wid in order(row):
                        return mid
                return None
            mid = wait(enrolled, "New window was not automatically enrolled")
            if monitor_id is None:
                monitor_id = mid
            assert mid == monitor_id, "Fixture windows must share a monitor/Space"
            m, row = settle()
            column = next(c for c in row["columns"] if wid in c["windows"])
            # Initial app/font setup can resize a window after the first CG
            # inventory sample. Compare with the independent geometry snapshot
            # retained by the daemon before it performs any AX resize.
            adopted = cli("status")["original_geometry"][str(wid)]
            assert adopted["pid"] == p.pid
            expected = min(adopted["bounds"]["width"], m["viewport"]["width"] - padding("left", "horizontal_margin") - padding("right", "horizontal_margin"))
            assert abs(column["width"]-expected) < 2, (column, expected)
            print("Auto-enrolled:", dict(id=wid, pid=p.pid, width=column["width"],
                                         first_sample_width=window["bounds"]["width"],
                                         adopted_width=adopted["bounds"]["width"]), flush=True)
            return wid

        def native_focus(wid):
            p = owned[wid]
            result = subprocess.run([str(HELPER), "--focus-fixture", str(wid), str(p.pid)],
                                    capture_output=True, text=True, timeout=3)
            assert result.returncode == 0, (result.stdout, result.stderr)
            wait(lambda: selected(context()[1]) == wid, "OS focus was not observed")
            return settle()

        def verify_now():
            m, row = settle()
            viewport = m["viewport"]
            x = viewport["x"] - row["scroll"]["position"]
            current_settings = snapshot()["settings"]
            top = current_settings.get("padding_top")
            top = current_settings["vertical_margin"] if top is None else top
            bottom = current_settings.get("padding_bottom")
            bottom = current_settings["vertical_margin"] if bottom is None else bottom
            gap = current_settings["gap"]
            weights = snapshot().get("row_weights", {})
            for c in row["columns"]:
                available = viewport["height"] - top - bottom - gap*(len(c["windows"])-1)
                total = sum(weights.get(str(wid), 1) for wid in c["windows"])
                y = viewport["y"] + top
                for ri, wid in enumerate(c["windows"]):
                    height = available * weights.get(str(wid), 1) / total
                    if wid not in owned or owned[wid].poll() is not None:
                        y += height + gap
                        continue
                    native = inspect(wid)
                    assert abs(native["transform"][4]+x) < 1 and abs(native["transform"][5]+y) < 1, native
                    assert abs(native["frame"]["width"]-c["width"]) < 2
                    assert abs(native["frame"]["height"]-height) < 2
                    left, right = max(x, viewport["x"]), min(x+c["width"], viewport["x"]+viewport["width"])
                    clip = native["clip_bounds"]
                    if right <= left:
                        assert clip[2:] == [0, 0], native
                    else:
                        assert abs(clip[0]-(left-x)) < 1 and abs(clip[2]-(right-left)) < 1, native
                    y += height + gap
                x += c["width"] + gap
            return m, row

        def verify():
            def applied():
                try:
                    return verify_now()
                except AssertionError:
                    return None
            return wait(applied, "Model/native geometry did not converge")

        def external_resize(wid, width, height):
            result = subprocess.run([str(HELPER), "--resize-fixture", str(wid), str(owned[wid].pid),
                                     str(width), str(height)], capture_output=True, text=True, timeout=3)
            assert result.returncode == 0, (result.stdout, result.stderr)
            wait(lambda: any(wid in col["windows"] and abs(col["width"]-width)<2 for col in context()[1]["columns"]),
                 "Owner-side resize was not adopted")
            return verify()

        try:
            a, b, c = spawn(0, 150), spawn(1, 210), spawn(2, 270)
            m, row = verify()
            assert sum(col["width"] for col in row["columns"]) + settings["gap"]*(len(row["columns"])-1) > m["viewport"]["width"]
            original_widths = {wid: next(col["width"] for col in row["columns"] if wid in col["windows"]) for wid in (a,b,c)}
            for _ in range(3):
                for wid in (a,c):
                    native_focus(wid)
                    _, current = verify()
                    assert {wid: next(col["width"] for col in current["columns"] if wid in col["windows"]) for wid in (a,b,c)} == original_widths
                    assert abs(inspect(wid)["clip_bounds"][2]-original_widths[wid]) < 2
            print("PASS: external native focus follows three scroll round trips; IDs, natural widths and real clips preserved", flush=True)
            native_focus(a)
            clip = inspect(c)["clip_bounds"]
            assert clip[2] >= 40, "The unfocused column needs a visible strip for the mouse focus probe"
            clicked = subprocess.run([str(HELPER), "--click-fixture", str(c), str(owned[c].pid)],
                                     capture_output=True, text=True, timeout=3)
            assert clicked.returncode == 0, (clicked.stdout, clicked.stderr)
            wait(lambda: selected(context()[1]) == c, "Mouse-driven native focus was not observed")
            verify()
            assert abs(inspect(c)["clip_bounds"][2]-original_widths[c]) < 2
            print("PASS: clicking an unfocused partially visible real window selects it and reveals its full column", flush=True)
            native_focus(a)
            cli("--monitor", monitor_id, "scroll", 123)
            saved = context()[1]["scroll"]["target"]
            time.sleep(0.8)
            assert context()[1]["scroll"]["target"] == saved
            print("PASS: repeated native focus observation preserves manual scroll", flush=True)
            native_focus(a)
            height = inspect(a)["frame"]["height"]
            external_resize(a, original_widths[a] + 120, height)
            time.sleep(0.7)
            assert abs(inspect(a)["frame"]["width"] - original_widths[a] - 120) < 2
            cli("-m", "window", "--resize", "left:120:0")
            verify()
            assert abs(inspect(a)["frame"]["width"] - original_widths[a]) < 2
            print("PASS: an external app resize updates the column and persists; yabai edge resize adjusts it relatively", flush=True)
            for operation, destination in [("--swap", c), ("--swap", c), ("--warp", c), ("--warp", b)]:
                cli("-m", "window", operation, destination)
                verify()
                assert set(order(context()[1])) == {a,b,c}
            assert order(context()[1]) == [a,b,c]
            cli("-m", "window", "--stack", b)
            verify()
            external_resize(a, original_widths[b], 450)
            assert abs(inspect(a)["frame"]["height"] - 450) < 2
            cli("-m", "window", "--resize", "bottom:0:50")
            verify()
            assert abs(inspect(a)["frame"]["height"] - 500) < 2
            cli("-m", "space", "--balance")
            verify()
            cli("--monitor", monitor_id, "unstack")
            cli("--monitor", monitor_id, "resize", original_widths[a])
            cli("-m", "window", "--warp", b)
            verify()
            cli("--monitor", monitor_id, "resize", "--ratio", 0.5)
            verify()
            cli("--monitor", monitor_id, "resize", original_widths[a])
            verify()
            queried = cli("-m", "query", "--windows")
            assert next(w for w in queried if w["id"] == a)["has-focus"]
            print("PASS: yabai swap/warp/stack/resize/balance and managed queries; native ratio resize; external stack-height adoption", flush=True)
            try:
                cli("-m", "config", "top_padding", 30)
                assert cli("-m", "config", "top_padding") == 30
                verify()
                assert abs(inspect(a)["transform"][5] + m["viewport"]["y"] + 30) < 1
                print("PASS: yabai spacing config updates real geometry without restarting", flush=True)
            finally:
                cli("-m", "config", "top_padding", padding("top", "vertical_margin"))
                verify()
            before = order(context()[1])
            d = spawn(3, 140)
            after = order(context()[1])
            expected = before.copy()
            expected.insert(before.index(a)+1, d)
            assert after == expected, (after, expected)
            print("PASS: a new real window is inserted directly right of the actual active column", flush=True)
            native_focus(c)
            x_before = inspect(c)["transform"][4]
            owned[b].terminate()
            owned[b].wait(timeout=5)
            wait(lambda: b not in order(context()[1]), "Closed nonfocused window remained managed")
            verify()
            assert abs(inspect(c)["transform"][4]-x_before) < 1
            print("PASS: closing a column left of focus preserves the surviving window's actual visual position", flush=True)
            owned[c].terminate()
            owned[c].wait(timeout=5)
            wait(lambda: c not in order(context()[1]), "Closed focused window remained managed")
            native_focus(d)
            verify()
            print("PASS: closing the focused app keeps the live daemon running and remaining windows usable", flush=True)
        finally:
            for p in processes:
                if p.poll() is None:
                    p.terminate()
                    p.wait(timeout=5)
        wait(lambda: not any(wid in order(context()[1]) for wid in owned), "Fixture cleanup did not finish")
        settle()
        print("PASS: all disposable terminals closed; the user's live WM remains running", flush=True)


if __name__ == "__main__":
    main(sys.argv[1])
