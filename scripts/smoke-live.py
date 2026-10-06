"""Exercise the real Rust daemon against six disposable Alacritty windows.

Only the windows created by this script are selected. Temporarily pauses an
already running Nix yabai service and restores it in finally. Never uses --all.
Run: nix develop -c python3 scripts/smoke-live.py /absolute/path/to/alacritty
"""
import json
import os
from pathlib import Path
import select
import socket
import queue
import re
import subprocess
import sys
import tempfile
import termios
import threading
import time
import tty

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/debug/ribbonwm"
INSPECT = ROOT / "native/build/test-window"
SOCKET = Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock")
LABEL = f"gui/{os.getuid()}/org.nixos.yabai"
PLIST = Path.home() / "Library/LaunchAgents/org.nixos.yabai.plist"
COUNT = 6


def terminal_child(index, destination):
    original = termios.tcgetattr(sys.stdin)
    tty.setraw(sys.stdin)
    counts = dict(x=0, other=0, mouse=0)
    pending = b""
    Path(destination).write_text(json.dumps(counts))
    print(f"\033[2J\033[HRibbonWM live QA: Alacritty column {index}\r\n"
          "Real terminal process. No shell commands are accepted here.\r\n"
          "\033[?1000h\033[?1006h", end="", flush=True)
    try:
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if select.select([sys.stdin], [], [], 0.2)[0]:
                data = os.read(sys.stdin.fileno(), 1024)
                if not data:
                    break
                counts["x"] += data.count(b"x")
                counts["other"] += len(data) - data.count(b"x")
                pending += data
                while match := re.search(rb"\x1b\[<0;\d+;\d+M", pending):
                    counts["mouse"] += 1
                    pending = pending[match.end():]
                pending = pending[-128:]
                Path(destination).write_text(json.dumps(counts))
                print(f"received x: {counts['x']}, mouse: {counts['mouse']}\r\n", end="", flush=True)
    finally:
        print("\033[?1000l\033[?1006l", end="", flush=True)
        termios.tcsetattr(sys.stdin, termios.TCSANOW, original)


def cli(*args):
    result = subprocess.run([str(BINARY), *map(str, args)], capture_output=True, text=True, timeout=5)
    if result.returncode:
        raise RuntimeError(f"{args}: {result.stderr.strip()}")
    value = json.loads(result.stdout)
    if isinstance(value, dict) and "ok" in value:
        assert value["ok"], value
    return value


def inspect(wid):
    value = json.loads(subprocess.check_output([str(INSPECT), str(wid)], text=True, timeout=2))
    assert value["errors"] == [0, 0] and value["clip_error"] == 0, value
    return value


def main(alacritty):
    if not os.environ.get("IN_NIX_SHELL"):
        raise RuntimeError("Run via nix develop")
    if SOCKET.exists():
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
            probe.settimeout(0.5)
            try:
                probe.connect(str(SOCKET))
            except ConnectionRefusedError:
                pass  # The daemon validates ownership and removes its stale socket.
            else:
                raise RuntimeError("Finish the existing WM daemon first")
    subprocess.run(["cargo", "build", "--locked", "--workspace"], cwd=ROOT, check=True)
    subprocess.run(["make", "-C", str(ROOT / "native"), "build/test-window"], check=True)
    assert cli("backend-status")["controlled"] == 0
    yabai_running = subprocess.run(["launchctl", "print", LABEL], stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL).returncode == 0
    paused = False
    terminals = []
    fixture = None
    daemon = None
    return_direction = None
    original_space = None
    monitor_id = None
    with tempfile.TemporaryDirectory(prefix="ribbon-live-") as directory:
        try:
            if yabai_running:
                subprocess.run(["launchctl", "bootout", LABEL], check=True)
                paused = True
                print("Paused org.nixos.yabai", flush=True)
            displays = cli("displays")
            pairs = [(a,b) for a in displays for b in displays if a["primary"] and a["id"] != b["id"]
                     and abs(a["frame"]["x"]+a["frame"]["width"]-b["frame"]["x"]) < 1
                     and min(a["viewport"]["y"]+a["viewport"]["height"], b["viewport"]["y"]+b["viewport"]["height"])
                     -max(a["viewport"]["y"], b["viewport"]["y"]) >= 650]
            probe = None
            if pairs:
                source,neighbor = pairs[0]
                seam = neighbor["frame"]["x"]
                top = max(source["viewport"]["y"],neighbor["viewport"]["y"])+180
                configuration = dict(lifetime=120, target=dict(x=seam-500,y=top), backdrops=[
                    dict(x=seam-600,y=top-100,width=600,height=600,surface="source"),
                    dict(x=seam,y=top-100,width=500,height=600,surface="neighbor")])
                # Create normal-level backgrounds BEFORE Alacritty, so they
                # cannot cover up a leaking terminal in the pixel comparison.
                fixture = subprocess.Popen([str(INSPECT),"--fixture",json.dumps(configuration)],
                    stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
                events = queue.Queue()
                def read_events():
                    for line in fixture.stdout:events.put(json.loads(line))
                threading.Thread(target=read_events,daemon=True).start()
                assert events.get(timeout=3).get("ready")
                probe = dict(source=source["id"],seam=seam,top=top,events=events)
            for i in range(COUNT):
                process = subprocess.Popen([alacritty, "--title", f"RibbonWM QA {i}", "-o", "window.opacity=1.0", "-e",
                    sys.executable, str(Path(__file__).resolve()), "--terminal-child", str(i),
                    str(Path(directory) / f"keys-{i}.json")], stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL)
                terminals.append(process)
                time.sleep(0.2)
            deadline = time.monotonic() + 8
            while True:
                inventory = cli("windows")
                chosen = [w for w in inventory if w["pid"] in [p.pid for p in terminals]
                          and w["onscreen"] and w["layer"] == 0 and w["bounds"]["width"] > 100]
                if len(chosen) == COUNT:
                    break
                if time.monotonic() > deadline or any(p.poll() is not None for p in terminals):
                    raise RuntimeError("Could not locate six disposable Alacritty windows")
                time.sleep(0.1)
            original = {w["id"]: inspect(w["id"]) for w in chosen}
            pids = {w["id"]: w["pid"] for w in chosen}
            print("Alacritty windows:", [{"id": w["id"], "pid": w["pid"]} for w in chosen], flush=True)
            # Keep the strip wider than the newly connected physical display.
            config = Path(directory) / "qa.toml"
            config.write_text("column_width = 1100.0\n")
            daemon_log = open(ROOT / "docs/live-daemon.txt", "w")
            daemon = subprocess.Popen([str(BINARY), "run", "--windows", ",".join(map(str, original)), "--config", str(config)],
                                      stdout=daemon_log, stderr=daemon_log)
            deadline = time.monotonic() + 5
            while not SOCKET.exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("Live daemon failed; see docs/live-daemon.txt")
                time.sleep(0.05)
            time.sleep(0.7)

            def verify():
                assert daemon.poll() is None, "Live daemon exited; see docs/live-daemon.txt"
                # AX initialization can delay the start of the scroll spring.
                # Reading status and native state in different processes while
                # it is still moving cannot give a coherent geometry snapshot.
                deadline = time.monotonic() + 6
                while True:
                    snapshot = cli("status")["state"]["monitors"]
                    scrolls = [m["contexts"][str(m["native_space"])]["scroll"] for m in snapshot.values()]
                    if all(abs(s["position"]-s["target"]) < 0.01 and abs(s["velocity"]) < 0.01 for s in scrolls):
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError("Scroll animation did not settle")
                    time.sleep(0.05)
                time.sleep(0.05)
                result = cli("status")
                assert result["mode"] == "live"
                monitors = result["state"]["monitors"]
                seen = []
                for monitor_id, monitor in monitors.items():
                    viewport = monitor["viewport"]
                    context = monitor["contexts"][str(monitor["native_space"])]
                    for row in [context]:
                        x = viewport["x"] - row["scroll"]["position"]
                        for column in row["columns"]:
                            height = (viewport["height"] - 16 * (len(column["windows"]) - 1)) / len(column["windows"])
                            for index, wid in enumerate(column["windows"]):
                                seen.append(wid)
                                native = inspect(wid)
                                y = viewport["y"] + index * (height + 16)
                                assert abs(native["transform"][4] + x) < 1 and abs(native["transform"][5] + y) < 1, native
                                assert abs(native["frame"]["width"] - column["width"]) < 2, (native, column["width"])
                                assert abs(native["frame"]["height"] - height) < 2, (native, height)
                                clip = native["clip_bounds"]
                                if x + column["width"] <= viewport["x"] or x >= viewport["x"] + viewport["width"]:
                                    assert clip[2:] == [0, 0], native
                                else:
                                    assert clip[2] > 0 and clip[3] > 0, native
                                    assert x + clip[0] >= viewport["x"] - 1
                                    assert x + clip[0] + clip[2] <= viewport["x"] + viewport["width"] + 1
                            x += column["width"] + 16
                assert sorted(seen) == sorted(original)
                assert cli("backend-status")["controlled"] == COUNT
                owners = [mid for mid, m in monitors.items() if any(
                    wid in original for c in m["contexts"][str(m["native_space"])]["columns"] for wid in c["windows"])]
                assert len(owners) == 1, "Space probe requires its fixture terminals on one monitor"
                return owners[0]

            monitor_id = verify()
            print("PASS: Rust daemon applies AX sizes and Dock transforms/clips to six Alacritty processes", flush=True)
            monitor = cli("status")["state"]["monitors"][monitor_id]
            row = monitor["contexts"][str(monitor["native_space"]) ]
            strip_width = sum(c["width"] for c in row["columns"])+16*(len(row["columns"])-1)
            assert strip_width > monitor["viewport"]["width"]*2
            print("Overflow setup:",dict(count=COUNT,column_width=1100,strip_width=strip_width,viewport=monitor["viewport"]),flush=True)

            def capture_neighbor(name):
                path=ROOT/f"docs/live-neighbor-{name}.png"
                region=(probe["seam"],probe["top"]-60,350,520)
                subprocess.run(["/usr/sbin/screencapture","-x","-R"+",".join(str(int(n)) for n in region),str(path)],check=True)
                return path

            baseline=capture_neighbor("baseline") if probe and probe["source"]==monitor_id else None
            cli("--monitor",monitor_id,"scroll",100000)
            verify()
            first=row["columns"][0]["windows"][0]
            initial_x=-inspect(first)["transform"][4]
            cli("focus-window",first)
            motion=[]
            captures=[]
            deadline=time.monotonic()+6
            while True:
                native=inspect(first)
                motion.append(-native["transform"][4])
                if baseline:
                    path=capture_neighbor(str(len(captures)))
                    captures.append(path)
                    fixture.stdin.write(json.dumps(dict(op="click",x=probe["seam"]+100,y=probe["top"]+200))+"\n")
                    fixture.stdin.flush()
                    hit=probe["events"].get(timeout=2)
                    assert hit.get("surface")=="neighbor" and abs(hit["x"]-100)<2 and abs(hit["y"]-300)<2,hit
                state=cli("status")["state"]["monitors"][monitor_id]
                scroll=state["contexts"][str(state["native_space"])]["scroll"]
                if scroll["position"]==scroll["target"] and scroll["velocity"]==0:break
                if time.monotonic()>deadline:raise RuntimeError("Rightward motion did not settle")
                time.sleep(0.08)
            verify()
            active=state["contexts"][str(state["native_space"])]
            print("Rightward focus diagnostics:",dict(requested=first,selected=active["columns"][active["focused_column"]]["windows"][active["focused_row"]],motion=motion),flush=True)
            assert initial_x < -monitor["viewport"]["width"]
            assert abs(motion[-1]-monitor["viewport"]["x"])<1
            assert all(b>=a-1 for a,b in zip(motion,motion[1:])),motion
            assert len(motion)>=3 and motion[-1]-initial_x>monitor["viewport"]["width"]
            right_edges=[]
            hidden=0
            row=cli("status")["state"]["monitors"][monitor_id]["contexts"][str(monitor["native_space"]) ]
            x=monitor["viewport"]["x"]
            edge=x+monitor["viewport"]["width"]
            for column in row["columns"]:
                native=inspect(column["windows"][0])
                if x<edge<x+column["width"]:
                    assert 0<native["clip_bounds"][2]<column["width"]
                    right_edges.append(dict(x=x,width=column["width"],clip=native["clip_bounds"]))
                elif x>=edge:
                    assert native["clip_bounds"][2:]==[0,0]
                    hidden+=1
                x+=column["width"]+16
            assert right_edges and hidden>=2
            print("PASS: columns move right through the physical right edge:",dict(first_x=motion,partial=right_edges,fully_hidden=hidden),flush=True)
            if baseline:
                # Check after motion has stopped: comparison must not stall the
                # daemon's animation or substitute for actual moving input tests.
                for path in captures:
                    compared=json.loads(subprocess.check_output([str(INSPECT),"--compare-images",str(baseline),str(path)],text=True))
                    assert compared["changed"]==0,(path,compared)
                print("PASS: all",len(captures),"moving-frame neighbor captures match baseline; neighbor receives every test click",flush=True)
            key_files=[Path(directory)/f"keys-{i}.json" for i in range(COUNT)]
            first,last=row["columns"][0]["windows"][0],row["columns"][-1]["windows"][0]
            expected_columns=row["columns"]

            def click_returned(wid):
                if fixture is None:return
                native=inspect(wid)
                clip=native["clip_bounds"]
                assert clip[2]>200 and clip[3]>200
                counts=[json.loads(f.read_text())["mouse"] for f in key_files]
                index=next(i for i,p in enumerate(terminals) if p.pid==pids[wid])
                fixture.stdin.write(json.dumps(dict(op="click",x=-native["transform"][4]+clip[0]+100,
                    y=-native["transform"][5]+clip[1]+200))+"\n")
                fixture.stdin.flush()
                deadline=time.monotonic()+2
                while json.loads(key_files[index].read_text())["mouse"]==counts[index]:
                    if time.monotonic()>deadline:raise RuntimeError(f"Returned terminal {wid} did not receive mouse input")
                    time.sleep(0.02)
                received=[json.loads(f.read_text())["mouse"] for f in key_files]
                expected=counts.copy();expected[index]+=1
                assert received==expected,(received,expected)

            # Three complete end-to-end round trips. Each destination starts
            # with an empty clip and must regain both its real surface and input.
            for cycle in range(3):
                for destination in (last,first):
                    assert inspect(destination)["clip_bounds"][2:]==[0,0], "Destination must start fully hidden"
                    cli("focus-window",destination)
                    verify()
                    after=cli("status")["state"]["monitors"][monitor_id]
                    after=after["contexts"][str(after["native_space"]) ]
                    assert after["columns"]==expected_columns
                    clip=inspect(destination)["clip_bounds"]
                    assert abs(clip[2]-1100)<2 and clip[3]>0
                    click_returned(destination)
                print("PASS: full scroll round trip",cycle+1,"keeps all six IDs/order/widths; both previously hidden terminals regain full clip and receive input",flush=True)
            # This separate probe has not yet passed on this machine. Do not
            # confuse successful AX focus calls with verified keyboard routing.
            if os.environ.get("RIBBON_TEST_KEYBOARD") == "1":
                target=chosen[0]
                cli("focus-window",target["id"])
                time.sleep(0.8)
                key_files=[Path(directory)/f"keys-{i}.json" for i in range(COUNT)]
                counts=[json.loads(f.read_text())["x"] for f in key_files]
                subprocess.run([str(INSPECT),"--key",str(target["pid"])],check=True)
                target_index=next(i for i,p in enumerate(terminals) if p.pid==target["pid"])
                deadline=time.monotonic()+1
                while json.loads(key_files[target_index].read_text())["x"]==counts[target_index]:
                    if time.monotonic()>deadline:raise RuntimeError("Focused Alacritty did not receive test key")
                    time.sleep(0.02)
                received=[json.loads(f.read_text())["x"] for f in key_files]
                expected=counts.copy();expected[target_index]+=1
                assert received==expected,(received,expected)
                print("PASS: keyboard input reaches only the focused actual Alacritty process",flush=True)
            else:
                print("SKIP: separate keyboard-routing probe (unverified; enable RIBBON_TEST_KEYBOARD=1)", flush=True)
            # Round trips finish at the first column. Begin this scenario at
            # column two so both directional targets exist; bounds now reject
            # instead of treating a missing neighbor as a successful no-op.
            cli("focus-window", expected_columns[1]["windows"][0])
            verify()
            for command in [("focus", "left"), ("focus", "right"), ("scroll", "120"),
                            ("resize", "800"), ("stack",), ("focus", "up"), ("focus", "down"), ("unstack",)]:
                cli("--monitor", monitor_id, *command)
                time.sleep(0.8)
                verify()
                print("PASS:", " ".join(command), flush=True)
            # Test real OS Space changes, using this user's enabled native hotkey.
            # The WM has no artificial row or workspace transition in this test.
            cli("--monitor", monitor_id, "scroll", "213")
            time.sleep(0.1)
            before = cli("status")["state"]["monitors"][monitor_id]
            original_space = before["native_space"]
            original_layout = before["contexts"][str(original_space)]
            assert original_layout["scroll"]["position"] > 0
            live_before={wid:inspect(wid) for wid in original}
            if fixture:
                viewport=before["viewport"]
                fixture.stdin.write(json.dumps(dict(op="move",x=viewport["x"]+50,y=viewport["y"]+100))+"\n")
                fixture.stdin.flush()
                assert probe["events"].get(timeout=2).get("moved")
                time.sleep(0.1)
            def native_space():
                return next(d["native_space"] for d in cli("displays") if d["id"] == monitor_id)
            subprocess.run([str(INSPECT), "--space", "next"], check=True)
            return_direction = "previous"
            deadline = time.monotonic() + 4
            while native_space() == original_space:
                if time.monotonic() > deadline:
                    raise RuntimeError("Native next-Space shortcut did not change the Space")
                time.sleep(0.1)
            time.sleep(1)
            away = cli("status")["state"]["monitors"][monitor_id]
            assert away["native_space"] != original_space
            assert away["contexts"][str(original_space)]["columns"] == original_layout["columns"]
            assert cli("backend-status")["controlled"] == COUNT
            for wid in original:
                assert inspect(wid)["frame"]==live_before[wid]["frame"]
                assert inspect(wid)["transform"]==live_before[wid]["transform"]
                assert inspect(wid)["clip_bounds"]==live_before[wid]["clip_bounds"]
            print("PASS: native Space change keeps existing sizes, transforms and clips without rebuilding", flush=True)
            subprocess.run([str(INSPECT), "--space", return_direction], check=True)
            deadline = time.monotonic() + 4
            while native_space() != original_space:
                if time.monotonic() > deadline:
                    raise RuntimeError("Native previous-Space shortcut did not return")
                time.sleep(0.1)
            return_direction = None
            time.sleep(1)
            verify()
            returned = cli("status")["state"]["monitors"][monitor_id]["contexts"][str(original_space)]
            assert returned["columns"] == original_layout["columns"]
            assert returned["scroll"]["target"] == original_layout["scroll"]["target"]
            assert abs(returned["scroll"]["position"] - original_layout["scroll"]["position"]) < 1
            print("PASS: native Space return restores column order, widths and manual scroll offset", flush=True)
            print("Retained native Space scroll:",dict(space=original_space,position=returned["scroll"]["position"],target=returned["scroll"]["target"]),flush=True)
            cli("quit")
            daemon.wait(timeout=5)
            daemon_log.close()
            assert daemon.returncode == 0 and not SOCKET.exists()
            for wid, baseline in original.items():
                restored = inspect(wid)
                assert restored["transform"] == baseline["transform"], (restored, baseline)
                for key in ("x", "y", "width", "height"):
                    assert abs(restored["frame"][key] - baseline["frame"][key]) < 2, (restored, baseline)
                assert restored["clip_bounds"] == baseline["clip_bounds"], (restored, baseline)
            assert cli("backend-status")["controlled"] == 0
            print("PASS: graceful quit restores all six actual app geometries, transforms and clips", flush=True)
        finally:
            if daemon is not None and daemon.poll() is None:
                daemon.terminate()
                daemon.wait(timeout=5)
            if return_direction and original_space is not None and monitor_id is not None and next(
                    d["native_space"] for d in cli("displays") if d["id"] == monitor_id) != original_space:
                subprocess.run([str(INSPECT), "--space", return_direction], check=True)
                time.sleep(1)
            for process in terminals:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
            if fixture is not None:
                if fixture.poll() is None:
                    fixture.stdin.write('{"op":"quit"}\n');fixture.stdin.flush()
                    try:fixture.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        fixture.terminate();fixture.wait(timeout=2)
                fixture.stdin.close();fixture.stdout.close()
            if paused:
                subprocess.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(PLIST)], check=True)
                print("Restored org.nixos.yabai", flush=True)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--terminal-child":
        terminal_child(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 2:
        main(sys.argv[1])
    else:
        raise SystemExit("Run: nix develop -c python3 scripts/smoke-live.py /absolute/path/to/alacritty")
