"""Read current account limits without starting a Codex conversation."""

import glob
import json
import os
import re
import select
import shutil
import signal
import subprocess
import time


def find_codex():
    found = shutil.which("codex")
    if found:
        return found
    home = os.path.expanduser("~")
    candidates = [os.path.join(home, rel) for rel in (
        ".local/bin/codex", ".npm-global/bin/codex", ".bun/bin/codex",
    )]
    # Non-interactive SSH shells often do not load nvm's PATH.
    candidates.extend(sorted(
        glob.glob(os.path.join(home, ".nvm/versions/node/*/bin/codex")),
        key=lambda path: tuple(int(n) for n in re.findall(r"\d+", path.split("/")[-3])),
        reverse=True,
    ))
    candidates.extend(("/usr/local/bin/codex", "/opt/homebrew/bin/codex"))
    return next((path for path in candidates if os.access(path, os.X_OK)), None)


def fetch_usage(program, timeout=12):
    env = os.environ.copy()
    # npm's Codex launcher uses /usr/bin/env node; use its matching runtime.
    env["PATH"] = os.path.dirname(program) + os.pathsep + env.get("PATH", "")
    child = subprocess.Popen(
        [program, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, env=env, start_new_session=True,
    )
    deadline = time.monotonic() + timeout
    pending = bytearray()

    def send(message):
        child.stdin.write((json.dumps(message) + "\n").encode())
        child.stdin.flush()

    def receive(request_id):
        while time.monotonic() < deadline:
            while b"\n" in pending:
                line, _, rest = pending.partition(b"\n")
                pending[:] = rest
                try:
                    message = json.loads(line)
                except (ValueError, UnicodeError):
                    continue
                if isinstance(message, dict) and message.get("id") == request_id:
                    if "error" in message:
                        raise RuntimeError("Codex usage request failed")
                    return message.get("result")
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([child.stdout], [], [], remaining)[0]:
                break
            chunk = os.read(child.stdout.fileno(), 8192)
            if not chunk:
                raise RuntimeError("Codex usage connection closed")
            pending.extend(chunk)
            if len(pending) > 1024 * 1024:
                raise RuntimeError("Codex usage response too large")
        raise TimeoutError("Codex usage lookup timed out")

    try:
        send({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "fastade", "version": "0.1.0"},
            "capabilities": {"experimentalApi": True},
        }})
        receive(1)
        send({"method": "initialized"})
        send({"id": 2, "method": "account/rateLimits/read", "params": {}})
        result = receive(2)
        if not isinstance(result, dict) or not isinstance(result.get("rateLimits"), dict):
            raise RuntimeError("Missing Codex account limits")
        return {"collectedAt": int(time.time()), "rateLimits": result["rateLimits"]}
    finally:
        # Kill the npm wrapper and its native child, including on timeout.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            # Some systems refuse killpg once the group has already exited.
            if child.poll() is None:
                child.kill()
        child.wait()
        child.stdin.close()
        child.stdout.close()


def main():
    program = find_codex()
    if program:
        print("CODEX_USAGE_JSON " + json.dumps(fetch_usage(program)))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Do not forward CLI diagnostics or credentials to the desktop client.
        pass
