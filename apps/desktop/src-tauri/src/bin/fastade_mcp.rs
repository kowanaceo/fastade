//! A thin MCP stdio server. An AI CLI (Claude Code, Codex, ...) spawns this
//! per-project, speaking MCP's newline-delimited JSON-RPC over stdio; every
//! tool call is forwarded over a local unix socket to the already-running
//! fastade desktop app, which actually owns the sessions. This binary holds
//! no session state itself — it is only a protocol bridge.
use fastade_desktop_lib::mcp::tool_definitions;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("report-activity") {
        // A CLI hook invocation, not an MCP stdio session: fire-and-forget a
        // status update, then exit immediately so the hook never blocks the
        // CLI it's attached to.
        let state = args.get(2).map(String::as_str).unwrap_or("idle");
        let payload = read_hook_payload(args.get(3).map(String::as_str));
        report_activity(hook_state(state, payload.as_deref()), payload.as_deref());
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
        "initialize" => Ok(initialize_result(load_global_notes().as_deref())),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(params),
        other => Err(json!({ "code": -32601, "message": format!("method not found: {other}") })),
    };

    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// MCP server instructions are added to the agent's context by clients such
/// as Codex. The global notes are always-on guidance, so they travel here
/// instead of waiting for the model to discover them with a tool.
fn initialize_result(instructions: Option<&str>) -> Value {
    let mut result = json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "fastade-mcp", "version": env!("CARGO_PKG_VERSION") }
    });
    if let Some(text) = instructions.filter(|text| !text.trim().is_empty()) {
        result["instructions"] = json!(text);
    }
    result
}

/// Initialization must remain usable when the desktop app is temporarily
/// unavailable. In that case the bridge starts without custom instructions;
/// once fastade is reachable, a newly started agent receives them normally.
fn load_global_notes() -> Option<String> {
    let response = send_to_app(&json!({ "op": "list_records", "profile_id": "global" })).ok()?;
    if response.get("ok") != Some(&Value::Bool(true)) {
        return None;
    }
    format_global_notes(response.get("records")?.as_array()?)
}

/// One `## title` section per note, oldest first.
fn format_global_notes(records: &[Value]) -> Option<String> {
    let sections: Vec<String> = records
        .iter()
        .filter(|record| record["kind"] == "note")
        .filter_map(|record| {
            let title = record["title"].as_str()?.trim();
            let content = record["content"].as_str().unwrap_or("").trim();
            Some(format!("## {title}\n\n{content}").trim_end().to_owned())
        })
        .collect();
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

/// Notes and tasks only: session control (sending input, stopping agents)
/// stays out of MCP on purpose.
const RECORD_TOOLS: [&str; 4] = [
    "list_records",
    "create_record",
    "update_record",
    "delete_record",
];

/// The tool arguments plus the caller's own fastade session, so a record
/// lands on the session the agent is working in without it knowing any ids.
fn record_request(name: &str, arguments: &Value) -> Value {
    let mut request = arguments.as_object().cloned().unwrap_or_default();
    request.insert("op".to_owned(), json!(name));
    request.remove("session_id");
    if let Ok(session_id) = std::env::var("FASTADE_SESSION_ID") {
        request.insert("session_id".to_owned(), json!(session_id));
    }
    Value::Object(request)
}

fn call_tool(params: Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| json!({ "code": -32602, "message": "missing tool name" }))?;
    let request = match name {
        "list_groups" => json!({ "op": "list_groups" }),
        "list_sessions" => json!({ "op": "list_sessions" }),
        "list_projects" => json!({ "op": "list_projects" }),
        record if RECORD_TOOLS.contains(&record) => {
            record_request(record, params.get("arguments").unwrap_or(&Value::Null))
        }
        other => {
            return Err(json!({ "code": -32602, "message": format!("unknown tool: {other}") }))
        }
    };

    let response =
        send_to_app(&request).map_err(|error| json!({ "code": -32000, "message": error }))?;
    let failed = response.get("ok") == Some(&Value::Bool(false));
    Ok(json!({
        "content": [{ "type": "text", "text": response.to_string() }],
        "isError": failed,
    }))
}

/// Claude Code's AskUserQuestion arrives as an ordinary PreToolUse (mapped to
/// `working`), yet it blocks on the user exactly like a permission prompt.
/// The hook payload on stdin names the tool, so promote that one to
/// `waiting`. stdin is only read for `working` hooks and never from a
/// terminal, so a manual run or Codex's `notify` callback cannot block here.
fn hook_state<'a>(state: &'a str, payload: Option<&str>) -> &'a str {
    if state != "working" {
        return state;
    }
    let tool = payload
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .and_then(|value| value.get("tool_name")?.as_str().map(str::to_owned));
    if is_user_question_tool(tool.as_deref()) {
        "waiting"
    } else {
        state
    }
}

fn read_hook_payload(argument: Option<&str>) -> Option<String> {
    use std::io::{IsTerminal, Read};
    if let Some(argument) = argument.filter(|value| value.trim_start().starts_with('{')) {
        return Some(argument.to_owned());
    }
    if io::stdin().is_terminal() {
        return None;
    }
    let mut payload = String::new();
    io::stdin()
        .take(1 << 20)
        .read_to_string(&mut payload)
        .ok()?;
    (!payload.trim().is_empty()).then_some(payload)
}

fn is_user_question_tool(tool: Option<&str>) -> bool {
    tool == Some("AskUserQuestion")
}

/// Silently does nothing if `$FASTADE_SESSION_ID` is unset (not run inside a
/// fastade session) or the app is not running — a hook must never fail the
/// CLI turn it is attached to just because the status update didn't land.
fn report_activity(state: &str, payload: Option<&str>) {
    let Ok(session_id) = std::env::var("FASTADE_SESSION_ID") else {
        return;
    };
    let hook = payload.and_then(|payload| serde_json::from_str::<Value>(payload).ok());
    let provider_session_id = hook.as_ref().and_then(|value| {
        value
            .get("session_id")
            .or_else(|| value.get("sessionId"))
            .and_then(Value::as_str)
    });
    let transcript_path = hook.as_ref().and_then(|value| {
        value
            .get("transcript_path")
            .or_else(|| value.get("transcriptPath"))
            .and_then(Value::as_str)
    });
    let effective_model = transcript_path.and_then(read_effective_model).or_else(|| {
        hook.as_ref().and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    });
    let request = json!({
        "op": "report_activity",
        "session_id": session_id,
        "state": state,
        "provider_session_id": provider_session_id,
        "effective_model": effective_model,
    });
    // CLIs wait for hook commands to exit, so a busy or wedged app must not
    // be able to stall the agent's turn: give up quickly instead.
    let _ = send_to_app_with_timeout(&request, Some(ACTIVITY_REPORT_TIMEOUT));
}

const ACTIVITY_REPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn send_to_app(request: &Value) -> Result<Value, String> {
    send_to_app_with_timeout(request, None)
}

fn read_effective_model(path: &str) -> Option<String> {
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom},
    };
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(1024 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    contents
        .lines()
        .filter_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            value
                .pointer("/payload/model")
                .or_else(|| value.pointer("/message/model"))
                .or_else(|| value.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .last()
}

#[cfg(unix)]
fn send_to_app_with_timeout(
    request: &Value,
    timeout: Option<std::time::Duration>,
) -> Result<Value, String> {
    use std::os::unix::net::UnixStream;

    // A remote host reaches the desktop app through an ssh-forwarded socket
    // whose path fastade puts in $FASTADE_SOCK when it opens the session.
    let path = match std::env::var_os("FASTADE_SOCK").filter(|value| !value.is_empty()) {
        Some(path) => std::path::PathBuf::from(path),
        None => fastade_desktop_lib::mcp::socket_path()
            .ok_or_else(|| "could not determine the fastade app's data directory".to_owned())?,
    };
    let mut stream = UnixStream::connect(&path)
        .map_err(|_| "the fastade desktop app is not running".to_owned())?;
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .map_err(|error| error.to_string())?;
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
fn send_to_app_with_timeout(
    _request: &Value,
    _timeout: Option<std::time::Duration>,
) -> Result<Value, String> {
    Err("the fastade MCP bridge is only available on macOS/Linux today".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn exposes_listing_and_record_tools_but_no_session_control() {
        let tools = tool_definitions();
        let tools = tools.as_array().expect("tool definitions must be an array");
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_groups",
                "list_sessions",
                "list_projects",
                "list_records",
                "create_record",
                "update_record",
                "delete_record"
            ]
        );

        let error = call_tool(json!({ "name": "send_message", "arguments": {} }))
            .expect_err("removed mutation tools must stay unavailable");
        assert_eq!(error["code"], -32602);
    }

    #[test]
    fn initialize_injects_the_global_notes() {
        let result = initialize_result(Some("Build apps accessibly.\nUse semantic HTML."));
        assert_eq!(
            result["instructions"],
            "Build apps accessibly.\nUse semantic HTML."
        );
    }

    #[test]
    fn global_notes_become_one_section_each() {
        let records = [
            json!({ "kind": "note", "title": "Apps", "content": "Ship a README.\n" }),
            json!({ "kind": "task", "title": "Ignored", "content": "x" }),
            json!({ "kind": "note", "title": "Web", "content": "" }),
        ];
        assert_eq!(
            format_global_notes(&records).unwrap(),
            "## Apps\n\nShip a README.\n\n## Web"
        );
        assert!(format_global_notes(&[]).is_none());
    }

    #[test]
    fn initialize_omits_empty_instructions() {
        assert!(initialize_result(None).get("instructions").is_none());
        assert!(initialize_result(Some("  \n")).get("instructions").is_none());
    }

    #[test]
    fn record_requests_carry_the_op_and_ignore_a_spoofed_session() {
        let request = record_request(
            "create_record",
            &json!({ "kind": "note", "title": "t", "session_id": "attacker" }),
        );
        assert_eq!(request["op"], "create_record");
        assert_eq!(request["title"], "t");
        assert_ne!(request["session_id"], "attacker");
        assert_eq!(
            record_request("list_records", &Value::Null)["op"],
            "list_records"
        );
    }

    #[test]
    fn reads_effective_models_from_codex_and_claude_transcripts() {
        let path = std::env::temp_dir().join(format!("fastade-model-{}.jsonl", std::process::id()));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-old\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-new\"}}\n"
            ),
        )
        .unwrap();
        assert_eq!(
            read_effective_model(path.to_str().unwrap()).as_deref(),
            Some("claude-new")
        );
        fs::remove_file(path).unwrap();
    }
}
