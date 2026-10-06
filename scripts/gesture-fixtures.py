"""Manual trackpad check against five owned Alacritty windows; expires in 5 min."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/debug/ribbonwm"
HELPER = ROOT / "native/build/test-window"


def cli(*args):
    return json.loads(subprocess.check_output([str(BINARY), *map(str, args)], timeout=5))


def main():
    assert os.environ.get("IN_NIX_SHELL")
    assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    assert cli("backend-status")["controlled"] == 0
    if "--automated" not in sys.argv[2:]:
        prompt=subprocess.run([str(HELPER),"--gesture-ready"])
        if prompt.returncode != 0:
            return
    display = next(d for d in cli("displays") if d["primary"])
    children, daemon = [], None
    with tempfile.TemporaryDirectory(prefix="ribbon-gestures-") as temporary:
        config = Path(temporary) / "config.toml"
        config.write_text("padding_left=24.0\npadding_right=24.0\npadding_top=24.0\npadding_bottom=24.0\n"
                          "gap=6.0\npreserve_window_width=true\ncenter_content=true\nframe_rate=120\n"
                          "gesture_scroll=true\ngesture_fingers=2\ngesture_modifier='alt'\ngesture_momentum=true\n"
                          "animation_curve='ease_in_out'\nanimation_duration=0.25\n")
        try:
            for i in range(5):
                body = f"import time;print('RIBBONWM GESTURE TEST — column {i + 1} / 5\\n\\nOption + 2 fingers left / right.\\nRelease your fingers and Option to test macOS momentum.\\n\\nOnly these five windows are managed.\\nThe test expires after five minutes.',flush=True);time.sleep(360)"
                p = subprocess.Popen([sys.argv[1], "--title", f"RibbonWM QA Gesture {i+1}",
                    "-o", "window.dimensions.columns=110", "-o", "window.dimensions.lines=25",
                    "-o", f"window.position.x={round((display['viewport']['x']+60)*display['scale'])}",
                    "-o", f"window.position.y={round((display['viewport']['y']+60)*display['scale'])}",
                    "-e", sys.executable, "-c", body], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                children.append(p)
                time.sleep(.2)
            deadline = time.monotonic() + 10
            while True:
                windows = [w for w in cli("windows") if w["pid"] in [p.pid for p in children]
                           and w["onscreen"] and w["layer"] == 0 and w["title"].startswith("RibbonWM QA Gesture ")]
                if len(windows) == 5:
                    break
                assert time.monotonic() < deadline and all(p.poll() is None for p in children)
                time.sleep(.1)
            with open(ROOT / "docs/gesture-fixture-daemon.txt", "w") as log:
                daemon = subprocess.Popen([str(BINARY), "run", "--windows", ",".join(str(w["id"]) for w in windows),
                    "--config", str(config), "--exclude-app", "com.openai.*", "--exclude-app", "ChatGPT*",
                    "--exclude-app", "com.apple.systempreferences"], stdout=log, stderr=log)
                deadline = time.monotonic() + 12
                while True:
                    assert daemon.poll() is None
                    try:
                        status = cli("status")
                        if len(status["placements"]) == 5:
                            break
                    except (subprocess.CalledProcessError, json.JSONDecodeError):
                        pass
                    assert time.monotonic() < deadline
                    time.sleep(.1)
                first = min(windows, key=lambda w: w["id"])
                subprocess.run([str(HELPER), "--focus-fixture", str(first["id"]), str(first["pid"])], check=True)
                time.sleep(.5)
                print(json.dumps(dict(ready=True, pid=daemon.pid, windows=[w["id"] for w in windows],
                                      monitor=display["id"], expires_seconds=300)), flush=True)
                if "--automated" in sys.argv[2:]:
                    def position():
                        state=cli("status")["state"]["monitors"][display["id"]]
                        return state["contexts"][str(state["native_space"])]["scroll"]["position"]
                    deadline = time.monotonic() + 10
                    while True:
                        state = cli("status")["state"]["monitors"][display["id"]]
                        scroll = state["contexts"][str(state["native_space"])]["scroll"]
                        if abs(scroll["position"] - scroll["target"]) < .01:
                            break
                        assert time.monotonic() < deadline, "Initial layout animation did not settle"
                        time.sleep(.05)
                    before=position()
                    for direct,tail in [(-240,-120),(240,120)]:
                        old=position()
                        subprocess.run([str(HELPER),"--scroll-fixture",str(first["id"]),str(first["pid"]),str(direct),str(tail)],check=True)
                        time.sleep(.15)
                        after=position()
                        assert abs(after-(old-direct-tail))<1,(old,direct,tail,after)
                        print(f"PASS: actual tap → Rust layout: {old:.1f} → {after:.1f}; modifier acquisition and native-phase momentum after key release",flush=True)
                    assert abs(position()-before)<1
                    cli("quit")
                    daemon.wait(timeout=8)
                    print("PASS: synthetic phase/momentum round trip; not a physical-finger smoothness claim",flush=True)
                    return
                deadline = time.monotonic() + 300
                while daemon.poll() is None and time.monotonic() < deadline and any(p.poll() is None for p in children):
                    time.sleep(.1)
        finally:
            if daemon and daemon.poll() is None:
                try:
                    cli("quit")
                    daemon.wait(timeout=8)
                except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                    daemon.terminate()
                    daemon.wait(timeout=8)
            for p in children:
                if p.poll() is None:
                    p.terminate()
            for p in children:
                p.wait(timeout=5)


if __name__ == "__main__":
    main()
