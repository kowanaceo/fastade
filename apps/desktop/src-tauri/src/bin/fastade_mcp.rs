//! A thin MCP stdio server. An AI CLI (Claude Code, Codex, ...) spawns this
//! per-project, speaking MCP's newline-delimited JSON-RPC over stdio; every
//! tool call is forwarded over a local unix socket to the already-running
//! fastade desktop app, which actually owns the sessions. This binary holds
//! no session state itself — it is only a protocol bridge.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("report-activity") {
        // A CLI hook invocation, not an MCP stdio session: fire-and-forget a
        // status update, then exit immediately so the hook never blocks the
        // CLI it's attached to.
        let state = args.get(2).map(String::as_str).unwrap_or("idle");
        report_activity(state);
        // Codex Stop/Interrupt hooks require successful command hooks to
        // return a JSON object. An empty object is also harmless for Claude
        // hooks and for Codex's legacy `notify` callback.
        println!("{{}}");
        return;
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(response) = handle_message(message) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
}

/// Returns `None` for a notification (no `id`), which gets no reply.
fn handle_message(message: Value) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message.get("method")?.as_str()?.to_owned();
    let params = message.get("params").cloned().unwrap_or(json!({}));

    let result = match method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "fastade-mcp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(params),
        other => Err(json!({ "code": -32601, "message": format!("method not found: {other}") })),
    };

    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_sessions",
            "description": "Return every saved fastade session, local or remote (SSH), overlaid with runtime session ID, agent, model, status, hook-reported activity and memory usage when available. Saved entries that have not been opened are returned as disconnected.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_projects",
            "description": "Return every project shown in fastade, one entry per saved project, with its name, current endpoint, project path, and paths remembered for other devices.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(params: Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| json!({ "code": -32602, "message": "missing tool name" }))?;
    let request = match name {
        "list_sessions" => json!({ "op": "list_sessions" }),
        "list_projects" => json!({ "op": "list_projects" }),
        other => {
            return Err(json!({ "code": -32602, "message": format!("unknown tool: {other}") }))
        }
    };

    let response =
        send_to_app(&request).map_err(|error| json!({ "code": -32000, "message": error }))?;
    Ok(json!({ "content": [{ "type": "text", "text": response.to_string() }] }))
}

/// Silently does nothing if `$FASTADE_SESSION_ID` is unset (not run inside a
/// fastade session) or the app is not running — a hook must never fail the
/// CLI turn it is attached to just because the status update didn't land.
fn report_activity(state: &str) {
    let Ok(session_id) = std::env::var("FASTADE_SESSION_ID") else {
        return;
    };
    let request = json!({ "op": "report_activity", "session_id": session_id, "state": state });
    let _ = send_to_app(&request);
}

#[cfg(unix)]
fn send_to_app(request: &Value) -> Result<Value, String> {
    use std::os::unix::net::UnixStream;

    let path = fastade_desktop_lib::mcp::socket_path()
        .ok_or_else(|| "could not determine the fastade app's data directory".to_owned())?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|_| "the fastade desktop app is not running".to_owned())?;
    let mut payload = request.to_string();
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut reader = io::BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn send_to_app(_request: &Value) -> Result<Value, String> {
    Err("the fastade MCP bridge is only available on macOS/Linux today".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_only_read_only_listing_tools() {
        let tools = tool_definitions();
        let tools = tools.as_array().expect("tool definitions must be an array");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "list_sessions");
        assert_eq!(tools[1]["name"], "list_projects");

        let error = call_tool(json!({ "name": "send_message", "arguments": {} }))
            .expect_err("removed mutation tools must stay unavailable");
        assert_eq!(error["code"], -32602);
    }
}
