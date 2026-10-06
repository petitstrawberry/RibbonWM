"""Reversible actual LaunchAgent start, mode QA, successful quit and unload."""
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def main(package, alacritty, module_config):
    binary = str(Path(package) / "bin/ribbonwm")
    label = "org.ribbonwm.qa.service"
    domain = f"gui/{os.getuid()}"
    target = domain + "/" + label
    assert subprocess.run(["launchctl", "print", target], capture_output=True).returncode != 0
    assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
    config = json.loads(Path(module_config).read_text())["agent"]
    # Remove null option defaults before serializing the actual plist.
    def prune(value):
        return {k: prune(v) for k, v in value.items() if v is not None} if isinstance(value, dict) else value
    config = prune(config)
    config["Label"] = label
    config["ProgramArguments"][0] = binary
    assert config["ProgramArguments"][1] == "service"
    config_path = config["ProgramArguments"][config["ProgramArguments"].index("--config") + 1]
    assert Path(config_path).is_file(), "Realize the module's generated TOML derivation before LaunchAgent QA"
    with tempfile.TemporaryDirectory(prefix="ribbon-launchd-") as temporary:
        plist = Path(temporary) / "service.plist"
        plist.write_bytes(plistlib.dumps(config))
        subprocess.run(["launchctl", "bootstrap", domain, str(plist)], check=True)
        try:
            until = time.monotonic() + 15
            while True:
                result = subprocess.run([binary, "status"], capture_output=True, text=True)
                if result.returncode == 0 and json.loads(result.stdout)["mode"] == "live":
                    break
                if time.monotonic() > until:
                    raise RuntimeError("LaunchAgent did not start a live daemon")
                time.sleep(.2)
            print("PASS: generated nix-darwin LaunchAgent arguments start the packaged Rust daemon", flush=True)
            subprocess.run([sys.executable, str(ROOT / "scripts/smoke-modes.py"), binary, alacritty], check=True)
            # Observe past launchd's throttle interval so a queued restart cannot
            # be mistaken for successful-quit suppression.
            until = time.monotonic() + config["ThrottleInterval"] + 1
            while time.monotonic() < until:
                assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists(), "Successful quit restarted the daemon"
                time.sleep(.2)
            status = subprocess.check_output(["launchctl", "print", target], text=True)
            assert "last exit code = 0" in status, status
            assert not Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists()
            print("PASS: successful quit leaves the service stopped; temporary LaunchAgent is unloaded", flush=True)
        finally:
            subprocess.run(["launchctl", "bootout", target], check=True)
            until = time.monotonic() + 15
            while Path(f"/tmp/ribbonwm-{os.getuid()}/wm.sock").exists() and time.monotonic() < until:
                time.sleep(.1)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("Usage: smoke-service.py PACKAGE ALACRITTY GENERATED_AGENT_JSON")
    main(sys.argv[1], sys.argv[2], sys.argv[3])
