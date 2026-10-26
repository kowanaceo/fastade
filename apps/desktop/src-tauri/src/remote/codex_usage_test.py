import json
import os
from pathlib import Path
import shlex
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import codex_usage


class CodexUsageTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def executable(self, path, script):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " -c " + shlex.quote(script) + '\n')
        path.chmod(0o755)
        return str(path)

    def test_noninteractive_nvm_discovery_uses_newest_runtime(self):
        for version in ("v9.0.0", "v22.0.0", "v24.13.0"):
            self.executable(self.root / f".nvm/versions/node/{version}/bin/codex", "")
        with patch.dict(os.environ, {"HOME": str(self.root), "PATH": "/nonexistent"}):
            self.assertEqual(codex_usage.find_codex(), str(self.root / ".nvm/versions/node/v24.13.0/bin/codex"))

    def test_live_lookup_waits_for_handshake_and_ignores_notifications(self):
        program = self.executable(self.root / "bin/codex", '''
import json, os, sys, time
assert os.environ["PATH"].split(os.pathsep)[0] == PROGRAM_DIRECTORY
assert json.loads(input())["method"] == "initialize"
print('{"id":1,"result":{}}', flush=True)
assert json.loads(input())["method"] == "initialized"
assert json.loads(input())["method"] == "account/rateLimits/read"
sys.stdout.write('unrelated output\\n{"method":"account/updated"}\\n' + json.dumps({
    "id": 2, "result": {"rateLimits": {
        "primary": {"usedPercent": 0, "resetsAt": 2000000000},
        "secondary": {"usedPercent": 0, "resetsAt": 2000500000}
    }}
}) + '\\n')
sys.stdout.flush()
time.sleep(30)
'''.replace('PROGRAM_DIRECTORY', repr(str(self.root / "bin"))))
        before = int(time.time())
        result = codex_usage.fetch_usage(program, timeout=2)
        self.assertGreaterEqual(result["collectedAt"], before)
        self.assertLessEqual(result["collectedAt"], int(time.time()))
        self.assertEqual(result["rateLimits"]["primary"]["usedPercent"], 0)

    def test_rpc_errors_and_missing_limits_produce_no_snapshot(self):
        for response in ({"id": 2, "error": {"message": "not signed in"}}, {"id": 2, "result": {}}):
            with self.subTest(response=response):
                program = self.executable(self.root / "codex", '''
import json
input()
print('{"id":1,"result":{}}', flush=True)
input()
input()
print(RESPONSE, flush=True)
'''.replace("RESPONSE", repr(json.dumps(response))))
                with self.assertRaises(RuntimeError):
                    codex_usage.fetch_usage(program, timeout=2)

    def test_timeout_kills_the_collector(self):
        pid_file = self.root / "collector.pid"
        program = self.executable(self.root / "codex", f'''
import os, time
from pathlib import Path
Path({str(pid_file)!r}).write_text(str(os.getpid()))
time.sleep(30)
''')
        with self.assertRaises(TimeoutError):
            codex_usage.fetch_usage(program, timeout=0.5)
        with self.assertRaises(ProcessLookupError):
            os.kill(int(pid_file.read_text()), 0)

    def test_connection_close_fails_without_waiting_for_timeout(self):
        program = self.executable(self.root / "codex", "pass")
        with self.assertRaises(RuntimeError):
            codex_usage.fetch_usage(program, timeout=2)


if __name__ == "__main__":
    unittest.main()
