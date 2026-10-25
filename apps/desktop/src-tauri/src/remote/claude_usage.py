import base64
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import sys
import termios
import time
import fcntl

ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]")


def find_claude():
    found = shutil.which("claude")
    if found:
        return found
    home = os.path.expanduser("~")
    for rel in (".local/bin/claude", ".claude/local/claude", ".npm-global/bin/claude", ".bun/bin/claude"):
        path = os.path.join(home, rel)
        if os.access(path, os.X_OK):
            return path
    for path in ("/usr/local/bin/claude", "/opt/homebrew/bin/claude"):
        if os.access(path, os.X_OK):
            return path
    return None


def trusted_directory():
    try:
        with open(os.path.expanduser("~/.claude.json")) as f:
            projects = json.load(f).get("projects", {})
        for path, meta in projects.items():
            if isinstance(meta, dict) and meta.get("hasTrustDialogAccepted") and os.path.isdir(path):
                return path
    except (OSError, ValueError, AttributeError):
        pass
    return os.path.expanduser("~")


def main():
    program = find_claude()
    if not program:
        print("CLAUDE_MISSING")
        return
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(trusted_directory())
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLORTERM"] = "truecolor"
        os.execv(program, [program])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
    raw = bytearray()

    def pump(timeout, ready):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if select.select([fd], [], [], 0.1)[0]:
                try:
                    data = os.read(fd, 8192)
                except OSError:
                    return False
                if not data:
                    return False
                if len(raw) < 512 * 1024:
                    raw.extend(data)
            text = ANSI.sub(" ", raw.decode("utf-8", "replace")).lower()
            if ready(text):
                return True
        return False

    try:
        pump(6, lambda t: "auto mode on" in t or "/effort" in t or "trust this folder" in t)
        if "trust this folder" in ANSI.sub(" ", raw.decode("utf-8", "replace")).lower():
            print("CLAUDE_TRUST")
            return
        os.write(fd, b"/usage")
        time.sleep(0.2)
        os.write(fd, b"\r")
        pump(7, lambda t: "% used" in t or "api usage billing" in t)
        pump(0.3, lambda t: False)
        print("CLAUDE_USAGE_B64 " + base64.b64encode(bytes(raw)).decode())
    finally:
        try:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        except OSError:
            pass


try:
    main()
except Exception:
    pass
