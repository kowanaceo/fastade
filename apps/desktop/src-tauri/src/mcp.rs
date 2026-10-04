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
    /// Notes and tasks. `profile_id` wins; otherwise the caller's own
    /// session (`$FASTADE_SESSION_ID`) picks the saved session they belong to.
    ListRecords {
        profile_id: Option<String>,
        session_id: Option<String>,
    },
    CreateRecord {
        profile_id: Option<String>,
        session_id: Option<String>,
        kind: String,
        title: String,
        content: Option<String>,
        status: Option<String>,
    },
    UpdateRecord {
        id: String,
        title: Option<String>,
        content: Option<String>,
        status: Option<String>,
    },
    DeleteRecord {
        id: String,
    },
    /// Sent by an AI CLI's own hook (via `fastade_mcp report-activity`) when
    /// it has one — `session_id` comes from `$FASTADE_SESSION_ID`, which the
    /// hook's own shell inherited from fastade at spawn time.
    ReportActivity {
        session_id: String,
        state: String,
        provider_session_id: Option<String>,
        effective_model: Option<String>,
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
        Request::ListRecords {
            profile_id,
            session_id,
        } => {
            let profile = match resolve_profile(app, &state, profile_id, session_id, false) {
                Ok(profile) => profile,
                Err(error) => return serde_json::json!({ "ok": false, "error": error }),
            };
            match crate::records::mcp_list_records(app, profile.as_deref()) {
                Ok(records) => serde_json::json!({ "ok": true, "records": records }),
                Err(error) => serde_json::json!({ "ok": false, "error": error }),
            }
        }
        Request::CreateRecord {
            profile_id,
            session_id,
            kind,
            title,
            content,
            status,
        } => {
            let result = resolve_profile(app, &state, profile_id, session_id, true)
                .and_then(|profile| profile.ok_or_else(|| "A saved session is required.".to_owned()))
                .and_then(|profile| {
                    crate::records::mcp_create_record(app, profile, kind, title, content, status)
                });
            records_changed_response(app, result.map(|record| ("record", record)))
        }
        Request::UpdateRecord {
            id,
            title,
            content,
            status,
        } => records_changed_response(
            app,
            crate::records::mcp_update_record(app, &id, title, content, status)
                .map(|record| ("record", record)),
        ),
        Request::DeleteRecord { id } => {
            match crate::records::delete_record(app, &id) {
                Ok(()) => {
                    let _ = app.emit("records-changed", ());
                    serde_json::json!({ "ok": true })
                }
                Err(error) => serde_json::json!({ "ok": false, "error": error }),
            }
        }
        Request::ReportActivity {
            session_id,
            state: activity,
            provider_session_id,
            effective_model,
        } => {
            if !matches!(activity.as_str(), "working" | "waiting" | "idle") {
                return serde_json::json!({ "ok": false, "error": "invalid activity state" });
            }
            crate::session::set_activity_override(&session_id, &activity, &state);
            let _ = crate::session::set_agent_metadata(
                &session_id,
                provider_session_id,
                effective_model,
                &state,
            );
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

/// An explicit `profile_id` must name a saved session; otherwise the calling
/// session's profile is used. `required` makes "no saved session" an error
/// instead of "everything".
#[cfg(unix)]
fn resolve_profile(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, crate::session::AppState>,
    profile_id: Option<String>,
    session_id: Option<String>,
    required: bool,
) -> Result<Option<String>, String> {
    if let Some(id) = profile_id.filter(|id| !id.trim().is_empty()) {
        let known = crate::saved_sessions::list_saved_sessions(app.clone())?
            .iter()
            .any(|profile| profile.id == id);
        return if known {
            Ok(Some(id))
        } else {
            Err("Unknown profile_id. Use list_sessions to find a saved session's profileId.".to_owned())
        };
    }
    if let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) {
        if let Some(profile) = crate::session::mcp_profile_for_session(app, state, &session_id)? {
            return Ok(Some(profile));
        }
    }
    if required {
        return Err(
            "No saved session to attach to: pass profile_id (from list_sessions) or call this from a saved fastade session."
                .to_owned(),
        );
    }
    Ok(None)
}

/// Replies with the changed record and tells the window to re-read and sync,
/// so an edit made through MCP reaches the server when signed in and simply
/// stays local otherwise.
#[cfg(unix)]
fn records_changed_response(
    app: &tauri::AppHandle,
    result: Result<(&'static str, crate::records::SessionRecord), String>,
) -> serde_json::Value {
    use tauri::Emitter;
    match result {
        Ok((key, record)) => {
            let _ = app.emit("records-changed", ());
            serde_json::json!({ "ok": true, key: record })
        }
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    }
}
