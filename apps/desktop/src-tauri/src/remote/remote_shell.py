"""Remote shell lifecycle helper. Standard library only; no account credentials.

The shell and everything it started end with the SSH connection; agents are
continued with their own resume commands, never kept alive on the host.
Run through python3 -c with one JSON argument; importing it has no side effects.
"""
import json
import os
import re
import signal
import subprocess
import sys
import time

SHELLS = {"sh", "bash", "zsh", "fish", "dash", "ksh", "login"}


def processes():
    # Read once per host, including stopped/background children and agents
    # whose foreground command is only the node launcher.
    output = subprocess.check_output(
        ["ps", "-axo", "pid=,ppid=,pgid=,tpgid=,stat=,comm=,args="], text=True)
    result = {}
    for line in output.splitlines():
        fields = line.strip().split(None, 6)
        if len(fields) < 6:
            continue
        pid, parent, group, foreground = map(int, fields[:4])
        result[pid] = dict(pid=pid, parent=parent, group=group,
                           foreground=foreground, state=fields[4],
                           command=os.path.basename(fields[5]),
                           args=fields[6] if len(fields) > 6 else fields[5])
    return result


def descendants(root, table):
    found = {root}
    while True:
        more = {pid for pid, row in table.items() if row["parent"] in found}
        if more <= found:
            return [table[pid] for pid in found if pid in table]
        found |= more


def agent_kind(row):
    command = row["command"].lstrip("-")
    if command in {"codex", "claude", "gemini"}:
        return command
    if command in {"node", "nodejs", "bun"}:
        args = row["args"]
        for kind, package in [("claude", "claude-code"), ("codex", "@openai/codex"),
                              ("gemini", "gemini-cli")]:
            if package in args or re.search(r"/" + kind + r"(?:\s|$|\.js)", args):
                return kind
    return None


def launch(options):
    """Preflight the connection's own shell before typing an agent command."""
    table = processes()
    owners = []
    for process in table.values():
        if not process["command"].lower().startswith("python"):
            continue
        try:
            args = json.loads('{"action":' + process["args"].rsplit(' {"action":', 1)[1])
            if args.get("action") == "connect" and args.get("runtimeId") == options["runtimeId"]:
                owners.append(process)
        except (IndexError, ValueError):
            pass
    if not owners:
        raise RuntimeError("Remote shell is still connecting; retry after its prompt appears")
    tree = descendants(owners[0]["pid"], table)
    agents = [dict(pid=p["pid"], cli=agent_kind(p), state=p["state"])
              for p in tree if agent_kind(p) and not p["state"].startswith("Z")]
    if agents:
        if all(p["state"].startswith("T") for p in agents) and len({p["cli"] for p in agents}) == 1:
            return dict(action="resumeDirect", cli=agents[0]["cli"])
        return dict(action="existing", cli=agents[0]["cli"])
    if any(p["command"].lstrip("-") not in SHELLS and p["pid"] != owners[0]["pid"]
           and not p["state"].startswith("Z") for p in tree):
        raise RuntimeError("This shell has a running job. Return to its prompt before starting an agent.")
    return dict(action="direct", cli=options["cli"])


def direct_shell():
    """Own the login shell and its descendants for the SSH PTY lifetime.

    Subreaping on Linux lets us reap orphaned grandchildren, including
    stopped agents. Deliberately daemonized, unrelated global services are
    outside this connection's ownership.
    """
    if sys.platform.startswith("linux"):
        import ctypes
        ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)  # PR_SET_CHILD_SUBREAPER
    ending = [False]
    for event in [signal.SIGHUP, signal.SIGTERM, signal.SIGINT]:
        signal.signal(event, lambda *_: ending.__setitem__(0, True))
    child = subprocess.Popen([os.environ.get("SHELL", "/bin/sh"), "-l"])
    try:
        while child.poll() is None and not ending[0]:
            time.sleep(0.1)
    finally:
        owned = [p["pid"] for p in descendants(os.getpid(), processes()) if p["pid"] != os.getpid()]
        for event in [signal.SIGCONT, signal.SIGTERM, signal.SIGKILL]:
            for pid in reversed(owned):
                try:
                    os.kill(pid, event)
                except ProcessLookupError:
                    pass
            if event != signal.SIGKILL:
                time.sleep(0.15)
        child.wait()
        # Reap any adopted children, never unrelated existing daemons.
        while True:
            try:
                os.waitpid(-1, 0)
            except ChildProcessError:
                break


def connect(options):
    directory = options.get("directory", "~")
    if directory == "~":
        directory = os.path.expanduser("~")
    os.chdir(directory)
    direct_shell()


def main(options):
    action = options["action"]
    if action == "connect":
        connect(options)
        return
    if action == "launch":
        print(json.dumps(launch(options)))
        return
    raise RuntimeError("Unknown remote shell action")


if __name__ == "__main__":
    main(json.loads(sys.argv[1]))
