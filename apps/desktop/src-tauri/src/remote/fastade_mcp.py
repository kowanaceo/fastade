#!/usr/bin/env python3
"""fastade's bridge for AI CLIs running on a remote (SSH) host.

fastade forwards its desktop app's local socket to this host when it opens the
session and exports FASTADE_SOCK / FASTADE_SESSION_ID in the remote shell, so
this script holds no credentials: every request is relayed to the desktop app,
which owns the notes, tasks and sessions (and signs in to the account).

  fastade-mcp                         MCP stdio server (what the CLI spawns)
  fastade-mcp report-activity STATE   CLI hook: working | waiting | idle

Standard library only; mirrors src/bin/fastade_mcp.rs.
"""
import glob
import json
import os
import socket
import sys

VERSION = "1"


def live_sockets():
    """Forwarded app sockets that still accept connections, newest first."""
    found = []
    for path in glob.glob("/tmp/fastade-*.sock"):
        try:
            if os.stat(path).st_uid != os.getuid():
                continue
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                probe.settimeout(2)
                probe.connect(path)
            found.append((os.stat(path).st_mtime, path))
        except OSError:
            continue
    return [path for _, path in sorted(found, reverse=True)]


def session_socket():
    """$FASTADE_SOCK, or, for a CLI whose daemon started outside the session
    shell (codex app-server) and so never inherited it, the newest live socket."""
    path = os.environ.get("FASTADE_SOCK")
    if path:
        return path
    sockets = live_sockets()
    return sockets[0] if sockets else None


def session_id():
    """$FASTADE_SESSION_ID, or the id in the socket name when only one session
    is connected (with several, a guess could file a note under the wrong one)."""
    value = os.environ.get("FASTADE_SESSION_ID")
    if value:
        return value
    sockets = live_sockets()
    if len(sockets) == 1:
        return os.path.basename(sockets[0])[len("fastade-"):-len(".sock")]
    return None


def send(request):
    path = session_socket()
    if not path:
        raise RuntimeError("not running inside a fastade session")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(10)
        try:
            sock.connect(path)
        except OSError:
            raise RuntimeError("the fastade desktop app is not reachable (is the session still connected?)")
        sock.sendall((json.dumps(request) + "\n").encode())
        data = b""
        while not data.endswith(b"\n"):
            chunk = sock.recv(65536)
            if not chunk:
                break
            data += chunk
    return json.loads(data.decode())


# ---- MCP stdio server -------------------------------------------------------

_tools = None


def tools():
    global _tools
    if _tools is None:
        reply = send({"op": "tools"})
        _tools = reply.get("tools", []) if reply.get("ok") else []
    return _tools


def call_tool(params):
    name = params.get("name")
    if name not in [tool["name"] for tool in tools()]:
        return None, {"code": -32602, "message": "unknown tool: %s" % name}
    request = dict(params.get("arguments") or {})
    request.pop("session_id", None)
    current = session_id()
    if current:
        request["session_id"] = current
    request["op"] = name
    try:
        reply = send(request)
    except Exception as error:  # noqa: BLE001 - reported to the CLI as an MCP error
        return None, {"code": -32000, "message": str(error)}
    return {
        "content": [{"type": "text", "text": json.dumps(reply)}],
        "isError": reply.get("ok") is False,
    }, None


def handle(message):
    if "id" not in message or "method" not in message:
        return None  # a notification gets no reply
    method, params = message["method"], message.get("params") or {}
    result, error = None, None
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fastade-mcp", "version": VERSION},
        }
    elif method == "tools/list":
        try:
            result = {"tools": tools()}
        except Exception as exc:  # noqa: BLE001
            error = {"code": -32000, "message": str(exc)}
    elif method == "tools/call":
        result, error = call_tool(params)
    elif method == "ping":
        result = {}
    else:
        error = {"code": -32601, "message": "method not found: %s" % method}
    reply = {"jsonrpc": "2.0", "id": message["id"]}
    reply["error" if error else "result"] = error or result
    return reply


def serve():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            reply = handle(json.loads(line))
        except Exception:  # noqa: BLE001 - never die on one bad message
            continue
        if reply is not None:
            sys.stdout.write(json.dumps(reply) + "\n")
            sys.stdout.flush()


# ---- CLI hook ---------------------------------------------------------------


def read_payload(argument):
    if argument and argument.lstrip().startswith("{"):
        return argument
    if sys.stdin.isatty():
        return None
    payload = sys.stdin.read(1 << 20)
    return payload if payload.strip() else None


def effective_model(path):
    """The model named in the last line of a transcript's final megabyte."""
    try:
        with open(path, "rb") as handle:
            handle.seek(0, os.SEEK_END)
            handle.seek(max(0, handle.tell() - 1024 * 1024))
            tail = handle.read().decode("utf-8", "replace")
    except OSError:
        return None
    model = None
    for line in tail.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if not isinstance(value, dict):
            continue
        found = (
            (value.get("payload") or {}).get("model")
            or (value.get("message") or {}).get("model")
            or value.get("model")
        )
        if isinstance(found, str):
            model = found
    return model


def report_activity(state, argument):
    payload = read_payload(argument)
    hook = None
    if payload:
        try:
            hook = json.loads(payload)
        except ValueError:
            hook = None
    hook = hook if isinstance(hook, dict) else {}
    # AskUserQuestion blocks on the user exactly like a permission prompt.
    if state == "working" and hook.get("tool_name") == "AskUserQuestion":
        state = "waiting"
    session_id = os.environ.get("FASTADE_SESSION_ID")
    if not session_id:
        return
    transcript = hook.get("transcript_path") or hook.get("transcriptPath")
    model = (effective_model(transcript) if transcript else None) or hook.get("model")
    try:
        send({
            "op": "report_activity",
            "session_id": session_id,
            "state": state,
            "provider_session_id": hook.get("session_id") or hook.get("sessionId"),
            "effective_model": model if isinstance(model, str) else None,
        })
    except Exception:  # noqa: BLE001 - a hook must never fail the CLI turn
        pass


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "report-activity":
        report_activity(
            sys.argv[2] if len(sys.argv) > 2 else "idle",
            sys.argv[3] if len(sys.argv) > 3 else None,
        )
        print("{}")  # Codex requires a JSON object from command hooks.
        return
    serve()


if __name__ == "__main__":
    main()
