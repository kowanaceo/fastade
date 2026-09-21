use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::thread;

/// Local IPC socket other processes — mainly the `fastade_mcp` stdio bridge
/// an AI CLI's own MCP client spawns — use to reach the sessions this
/// running desktop app owns. Lives in the same directory Tauri already uses
/// for `sessions.json`, so it needs no extra permission prompt and no
/// separate cleanup story. Computed without any live `AppHandle` (plain
/// `dirs` lookups) so the standalone `fastade_mcp` binary can compute the
/// exact same path without linking against a running Tauri app.
pub fn socket_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()?.join("Library/Application Support")
    } else {
        dirs::data_dir()?
    };
    Some(base.join("com.fastade.desktop").join("fastade.sock"))
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    ListSessions,
    ListProjects,
    /// Sent by an AI CLI's own hook (via `fastade_mcp report-activity`) when
    /// it has one — `session_id` comes from `$FASTADE_SESSION_ID`, which the
    /// hook's own shell inherited from fastade at spawn time.
    ReportActivity {
        session_id: String,
        state: String,
    },
}

#[cfg(unix)]
pub fn start(app: tauri::AppHandle) {
    use std::os::unix::net::UnixListener;

    let Some(path) = socket_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A stale socket from a previous run (e.g. the app was killed) would
    // otherwise make `bind` fail forever.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            thread::spawn(move || handle_client(app, stream));
        }
    });
}

#[cfg(not(unix))]
pub fn start(_app: tauri::AppHandle) {
    // Windows would need a named pipe instead of a unix socket; the MCP
    // bridge is local-only for now and not wired up there yet.
}

#[cfg(unix)]
fn handle_client(app: tauri::AppHandle, stream: std::os::unix::net::UnixStream) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(reader_stream);
    let mut writer = stream;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(&app, request),
            Err(error) => {
                serde_json::json!({ "ok": false, "error": format!("bad request: {error}") })
            }
        };
        let mut payload = response.to_string();
        payload.push('\n');
        if writer.write_all(payload.as_bytes()).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
fn handle_request(app: &tauri::AppHandle, request: Request) -> serde_json::Value {
    use tauri::{Emitter, Manager};

    let state = app.state::<crate::session::AppState>();
    match request {
        Request::ListSessions => match crate::session::mcp_sessions(app, &state) {
            Ok(sessions) => serde_json::json!({ "ok": true, "sessions": sessions }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        },
        Request::ListProjects => match crate::saved_sessions::mcp_projects(app.clone()) {
            Ok(projects) => serde_json::json!({ "ok": true, "projects": projects }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        },
        Request::ReportActivity {
            session_id,
            state: activity,
        } => {
            if !matches!(activity.as_str(), "working" | "waiting" | "idle") {
                return serde_json::json!({ "ok": false, "error": "invalid activity state" });
            }
            crate::session::set_activity_override(&session_id, &activity, &state);
            // Keep the persisted snapshot for reloads, but also push the
            // transition to the visible window immediately. Polling alone made
            // a short turn look stuck in its previous colour for up to a second.
            let _ = app.emit(
                "terminal-event",
                serde_json::json!({
                    "sessionId": session_id,
                    "kind": "activity",
                    "content": activity,
                }),
            );
            serde_json::json!({ "ok": true })
        }
    }
}
