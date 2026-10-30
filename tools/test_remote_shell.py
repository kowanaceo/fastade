"""Remote shell lifecycle regressions.

python3 tools/test_remote_shell.py
To test on an SSH host, stage this file beside remote_shell.py and set
FASTADE_REMOTE_HELPER to the helper path.
"""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HELPER = Path(os.environ.get("FASTADE_REMOTE_HELPER", str(
    Path(__file__).resolve().parents[1] / "apps/desktop/src-tauri/src/remote/remote_shell.py")))
spec = importlib.util.spec_from_file_location("remote_shell", HELPER)
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class DirectShellTests(unittest.TestCase):
    def test_disconnect_reaps_running_and_stopped_children(self):
        with tempfile.TemporaryDirectory(prefix="fastade-direct-test-") as directory:
            pidfile = Path(directory) / "agent.pid"
            shell = Path(directory) / "shell"
            shell.write_text("#!/bin/sh\nsleep 60 &\nchild=$!\necho $child > " + shlex.quote(str(pidfile)) + "\nkill -STOP $child\nwait\n")
            shell.chmod(0o700)
            env = dict(os.environ, SHELL=str(shell))
            owner = subprocess.Popen([sys.executable, "-c", HELPER.read_text(), json.dumps({"action": "connect", "directory": directory})], env=env, start_new_session=True)
            try:
                deadline = time.time() + 3
                while not pidfile.exists() and time.time() < deadline:
                    time.sleep(.05)
                child = int(pidfile.read_text())
                os.kill(owner.pid, signal.SIGHUP)
                owner.wait(timeout=4)
                proc = helper.processes().get(child)
                self.assertTrue(proc is None or proc["state"].startswith("Z"))
            finally:
                if owner.poll() is None:
                    owner.kill()
                    owner.wait()
                try:
                    os.killpg(owner.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    unittest.main(verbosity=2)
