#!/usr/bin/env python3
"""Exercise the real helper, replacement, startup acknowledgment and cleanup.

Uses disposable copies of target/debug/Starter.app. Never replaces /Applications.
Run after scripts/bundle-macos.sh --debug.
"""
import json
import os
import plistlib
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
bundle = root / "target/debug/Starter.app"
assert bundle.is_dir(), "Build the debug bundle first"
with (bundle / "Contents/Info.plist").open("rb") as source:
    version = plistlib.load(source)["CFBundleShortVersionString"]
with tempfile.TemporaryDirectory(prefix="starter-updater-test-") as test_dir:
    test = Path(test_dir)
    destination = test / "Installed/Starter.app"
    staging = test / ".staging"
    staging.mkdir()
    shutil.copytree(bundle, destination)
    payload = staging / "Starter.app"
    shutil.copytree(bundle, payload)
    workspace = test / "helper"
    workspace.mkdir()
    helper = workspace / "update-helper"
    shutil.copy2(bundle / "Contents/MacOS/starter", helper)
    report = test / "result.json"
    parent = subprocess.Popen(["/bin/sleep", "0.5"])
    plan = dict(mode="Bundle", pid=parent.pid, version=version,
                destination=str(destination), payload=str(payload),
                backup=str(staging / "Previous.app"), workspace=str(workspace),
                staging_root=str(staging), report=str(report))
    plan_path = workspace / "plan.json"
    plan_path.write_text(json.dumps(plan))
    process = subprocess.Popen([str(helper), "--apply-update", str(plan_path)])
    try:
        deadline = time.monotonic() + 45
        # Reap the simulated parent so kill(pid, 0) sees that it has exited.
        parent.wait(timeout=5)
        while not report.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        assert report.exists(), "Updater did not finish"
        result = json.loads(report.read_text())
        assert result["error"] is None, result["error"]
        process.wait(timeout=5)
        assert destination.is_dir() and not staging.exists(), "Replacement/backup cleanup failed"
        assert not workspace.exists(), "Helper cleanup failed"
        print("PASS: helper waited, swapped bundle, restarted, received startup acknowledgment and removed backup")
    finally:
        # Stop only processes running from this disposable test bundle.
        output = subprocess.check_output(["ps", "-axo", "pid=,command="], text=True)
        for row in output.splitlines():
            parts = row.strip().split(None, 1)
            if len(parts) == 2 and parts[1].startswith(str(destination / "Contents/MacOS/starter")):
                try:
                    os.kill(int(parts[0]), signal.SIGTERM)
                except ProcessLookupError:
                    pass
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
