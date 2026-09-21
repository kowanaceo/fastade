use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};
use tauri::{Emitter, Manager};
use uuid::Uuid;

const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;
const MAX_TRANSCRIPT_BYTES: usize = 2 * 1024 * 1024;
const SSH_KEEPALIVE_INTERVAL_SECONDS: &str = "30";
const SSH_KEEPALIVE_FAILURES: &str = "3";

pub struct AppState {
    sessions: Mutex<Vec<SessionSummary>>,
    terminals: Mutex<HashMap<String, Arc<TerminalHandle>>>,
    transcripts: Mutex<HashMap<String, String>>,
    sessions_path: PathBuf,
    /// "working"/"waiting"/"idle" reported by an AI CLI's own hook (via the
    /// `fastade_mcp report-activity` bridge), keyed by session id — ground
    /// truth from the CLI itself, when it has hooks, instead of the PTY-text
    /// heuristic the frontend falls back to otherwise.
    activity_overrides: Mutex<HashMap<String, String>>,
}

impl AppState {
    pub fn load(app: &tauri::AppHandle) -> Result<Self, String> {
        let sessions_path = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?
            .join("sessions.json");
        let mut sessions = read_sessions(&sessions_path)?;

        // A PTY cannot survive the app process. Keep its reusable metadata,
        // including the last AI agent/model so a pinned session can relaunch
        // the appropriate CLI in resume mode, but make the terminal itself
        // explicitly reconnectable.
        for session in &mut sessions {
            session.status = SessionStatus::Completed;
        }

        // A repeated crash/force-quit before a session ever reconnects can
        // leave several rows behind for the same project; once restored
        // they're all identical, disconnected placeholders (no transcript
        // survives a restart either), so keep only the newest — sessions
        // are prepended on creation, so the first match per project wins.
        let before = sessions.len();
        let mut seen = std::collections::HashSet::new();
        sessions.retain(|session| {
            seen.insert((session.endpoint.clone(), session.project_path.clone()))
        });
        let deduped = sessions.len() != before;

        let transcripts = sessions
            .iter()
            .map(|session| (session.id.clone(), String::new()))
            .collect();

        let state = Self {
            sessions: Mutex::new(sessions),
            terminals: Mutex::new(HashMap::new()),
            transcripts: Mutex::new(transcripts),
            sessions_path,
            activity_overrides: Mutex::new(HashMap::new()),
        };
        if deduped {
            let _ = persist_sessions(&state);
        }
        Ok(state)
    }
}

struct TerminalHandle {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// PID of the shell this session's PTY runs (its `ssh` client for a
    /// remote session, not the remote-side process). Used only to size up
    /// this session's local memory footprint for the UI badge.
    pid: Option<u32>,
}

/// A session is an interactive shell on a device at a folder. Which AI
/// agent (if any) is running inside it is mutable state the user changes at
/// will, not launch configuration.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub cli: Option<CliKind>,
    pub model: Option<String>,
    pub endpoint: String,
    pub project_path: String,
    pub status: SessionStatus,
}

/// Read-only session snapshot exposed by fastade MCP. Terminal transcripts
/// and credentials are intentionally excluded.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpSessionSummary {
    id: String,
    profile_id: Option<String>,
    session_id: Option<String>,
    saved: bool,
    title: String,
    cli: Option<CliKind>,
    model: Option<String>,
    endpoint: String,
    project_path: String,
    status: McpSessionStatus,
    agent_running: bool,
    activity: Option<String>,
    memory_bytes: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum McpSessionStatus {
    Running,
    Completed,
    Failed,
    Disconnected,
}

impl From<SessionStatus> for McpSessionStatus {
    fn from(status: SessionStatus) -> Self {
        match status {
            SessionStatus::Running => Self::Running,
            SessionStatus::Completed => Self::Completed,
            SessionStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CliKind {
    Codex,
    Gemini,
    Claude,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    title: String,
    endpoint: String,
    project_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalEvent {
    session_id: String,
    kind: &'static str,
    content: String,
}

#[tauri::command]
pub fn list_sessions(state: tauri::State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    state
        .sessions
        .lock()
        .map(|value| value.clone())
        .map_err(|_| "session store is unavailable".to_owned())
}

/// The most recently created local session's folder, if any. Used as a
/// working directory for helper CLI invocations (like usage lookups) so they
/// launch somewhere the user has very likely already used Claude Code and
/// trusted, instead of an unpredictable inherited directory.
pub fn most_recent_local_path(state: &AppState) -> Option<String> {
    state
        .sessions
        .lock()
        .ok()?
        .iter()
        .find(|session| session.endpoint == "local")
        .map(|session| session.project_path.clone())
}

#[tauri::command]
pub fn create_session(
    input: CreateSessionInput,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<SessionSummary, String> {
    validate_input(&input)?;
    let session = SessionSummary {
        id: Uuid::new_v4().to_string(),
        title: input.title,
        cli: None,
        model: None,
        endpoint: input.endpoint,
        project_path: input.project_path,
        status: SessionStatus::Running,
    };
    state
        .sessions
        .lock()
        .map_err(|_| "session store is unavailable".to_owned())?
        .insert(0, session.clone());
    state
        .transcripts
        .lock()
        .map_err(|_| "transcript store is unavailable".to_owned())?
        .insert(session.id.clone(), String::new());
    if let Err(error) = persist_sessions(&state) {
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.retain(|item| item.id != session.id);
        }
        if let Ok(mut transcripts) = state.transcripts.lock() {
            transcripts.remove(&session.id);
        }
        return Err(error);
    }
    if let Err(error) = spawn_terminal(&session, &state, app) {
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.retain(|item| item.id != session.id);
        }
        if let Ok(mut transcripts) = state.transcripts.lock() {
            transcripts.remove(&session.id);
        }
        let _ = persist_sessions(&state);
        return Err(error);
    }
    Ok(session)
}

#[tauri::command]
pub fn write_terminal(
    session_id: String,
    data: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    write_bytes(&session_id, &data, &state)
}

fn write_bytes(
    session_id: &str,
    data: &str,
    state: &tauri::State<'_, AppState>,
) -> Result<(), String> {
    let terminal = get_terminal(session_id, state)?;
    let mut writer = terminal
        .writer
        .lock()
        .map_err(|_| "terminal writer is unavailable".to_owned())?;
    writer
        .write_all(data.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|error| format!("terminal input failed: {error}"))
}

#[tauri::command]
pub fn resize_terminal(
    session_id: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let terminal = get_terminal(&session_id, &state)?;
    let result = terminal
        .master
        .lock()
        .map_err(|_| "terminal is unavailable".to_owned())?
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(10),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("terminal resize failed: {error}"));
    result
}

#[tauri::command]
pub fn terminal_snapshot(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    state
        .transcripts
        .lock()
        .map_err(|_| "transcript store is unavailable".to_owned())?
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "session not found".to_owned())
}

#[tauri::command]
pub fn update_session_context(
    session_id: String,
    title: String,
    project_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<SessionSummary, String> {
    if title.trim().is_empty() || project_path.trim().is_empty() {
        return Err("title and project path are required".to_owned());
    }
    let updated = {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "session store is unavailable".to_owned())?;
        let session = sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "session not found".to_owned())?;
        session.title = title;
        session.project_path = project_path;
        session.clone()
    };
    persist_sessions(&state)?;
    Ok(updated)
}

/// Records which agent is currently running inside the session's shell
/// (`None` once it exits back to the prompt), for the session header.
#[tauri::command]
pub fn update_session_agent(
    session_id: String,
    cli: Option<CliKind>,
    model: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<SessionSummary, String> {
    let updated = {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "session store is unavailable".to_owned())?;
        let session = sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "session not found".to_owned())?;
        session.cli = cli;
        session.model = model.filter(|value| !value.trim().is_empty());
        session.clone()
    };
    persist_sessions(&state)?;
    Ok(updated)
}

#[tauri::command]
pub fn interrupt_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<SessionSummary, String> {
    write_terminal(session_id.clone(), "\u{3}".to_owned(), state.clone())?;
    find_session(&session_id, &state)
}

#[tauri::command]
pub fn reconnect_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<SessionSummary, String> {
    if state
        .terminals
        .lock()
        .map_err(|_| "terminal store is unavailable".to_owned())?
        .contains_key(&session_id)
    {
        return Err("session is already running".to_owned());
    }

    let session = {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "session store is unavailable".to_owned())?;
        let session = sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| "session not found".to_owned())?;
        session.status = SessionStatus::Running;
        session.clone()
    };

    if let Ok(mut transcripts) = state.transcripts.lock() {
        transcripts
            .entry(session_id.clone())
            .or_default()
            .push_str("\r\n\x1b[2m── reconnected ──\x1b[0m\r\n");
    }

    if let Err(error) = spawn_terminal(&session, &state, app) {
        if let Ok(mut sessions) = state.sessions.lock() {
            if let Some(session) = sessions.iter_mut().find(|item| item.id == session_id) {
                session.status = SessionStatus::Failed;
            }
        }
        let _ = persist_sessions(&state);
        return Err(error);
    }
    persist_sessions(&state)?;
    Ok(session)
}

#[tauri::command]
pub fn close_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(terminal) = state
        .terminals
        .lock()
        .map_err(|_| "terminal store is unavailable".to_owned())?
        .remove(&session_id)
    {
        terminal
            .killer
            .lock()
            .map_err(|_| "terminal process is unavailable".to_owned())?
            .kill()
            .map_err(|error| format!("terminal stop failed: {error}"))?;
    }
    {
        let mut sessions = state
            .sessions
            .lock()
            .map_err(|_| "session store is unavailable".to_owned())?;
        let previous_len = sessions.len();
        sessions.retain(|session| session.id != session_id);
        if sessions.len() == previous_len {
            return Err("session not found".to_owned());
        }
    }
    persist_sessions(&state)?;
    if let Ok(mut transcripts) = state.transcripts.lock() {
        transcripts.remove(&session_id);
    }
    if let Ok(mut overrides) = state.activity_overrides.lock() {
        overrides.remove(&session_id);
    }
    if let Some(window) = app.get_webview_window(&format!("session-{session_id}")) {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn spawn_terminal(
    session: &SessionSummary,
    state: &tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to create PTY: {error}"))?;
    let managed_connection = session
        .endpoint
        .strip_prefix("managed:")
        .map(|id| crate::servers::resolve_managed_server(&app, id))
        .transpose()?
        .map(|(server, password)| ManagedConnection {
            host: server.host,
            port: server.port,
            username: server.username,
            key_path: match server.auth {
                crate::servers::AuthMethod::KeyFile { path } => Some(path),
                crate::servers::AuthMethod::Password => None,
            },
            password,
        });
    let (command, autofill_password) = build_command(session, managed_connection.as_ref())?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start shell: {error}"))?;
    let pid = child.process_id();
    let killer = child.clone_killer();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to create PTY reader: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to create PTY writer: {error}"))?;
    drop(pair.slave);
    let handle = Arc::new(TerminalHandle {
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
        killer: Mutex::new(killer),
        pid,
    });
    let autofill_handle = handle.clone();
    state
        .terminals
        .lock()
        .map_err(|_| "terminal store is unavailable".to_owned())?
        .insert(session.id.clone(), handle);

    let output_app = app.clone();
    let output_session_id = session.id.clone();
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut decoder = Utf8StreamDecoder::default();
        // A managed server's password (if any) is typed once, the moment an
        // `ssh` password prompt appears; a short rolling tail catches the
        // prompt even if it lands split across two reads.
        let mut password_sent = autofill_password.is_none();
        let mut prompt_tail = String::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let content = decoder.push(&buffer[..count]);
                    if !content.is_empty() {
                        if !password_sent {
                            prompt_tail.push_str(&content);
                            if prompt_tail.len() > 256 {
                                let cut = prompt_tail.len() - 256;
                                prompt_tail = prompt_tail[cut..].to_owned();
                            }
                            if prompt_tail.to_ascii_lowercase().contains("password:") {
                                if let (Some(password), Ok(mut writer)) =
                                    (autofill_password.as_deref(), autofill_handle.writer.lock())
                                {
                                    let _ = writer
                                        .write_all(format!("{password}\r").as_bytes())
                                        .and_then(|_| writer.flush());
                                }
                                password_sent = true;
                            }
                        }
                        record_and_emit(&output_app, &output_session_id, content);
                    }
                }
                Err(_) => break,
            }
        }
        let remaining = decoder.finish();
        if !remaining.is_empty() {
            record_and_emit(&output_app, &output_session_id, remaining);
        }
    });

    let wait_app = app;
    let wait_session_id = session.id.clone();
    thread::spawn(move || {
        let (status, content) = match child.wait() {
            Ok(exit) if exit.success() => (SessionStatus::Completed, "completed"),
            Ok(_) => (SessionStatus::Failed, "failed"),
            Err(_) => (SessionStatus::Failed, "failed"),
        };
        let app_state = wait_app.state::<AppState>();
        if let Ok(mut sessions) = app_state.sessions.lock() {
            if let Some(session) = sessions.iter_mut().find(|item| item.id == wait_session_id) {
                session.status = status;
            }
        }
        let _ = persist_sessions(&app_state);
        if let Ok(mut terminals) = app_state.terminals.lock() {
            terminals.remove(&wait_session_id);
        }
        if let Ok(mut overrides) = app_state.activity_overrides.lock() {
            overrides.remove(&wait_session_id);
        }
        emit_event(&wait_app, &wait_session_id, "status", content.to_owned());
    });
    Ok(())
}

/// A managed server's connection details, resolved (keychain lookup
/// included) before `build_command` runs, so `build_command` itself stays a
/// pure, easily-tested function that never needs a live Tauri app.
struct ManagedConnection {
    host: String,
    port: u16,
    username: String,
    key_path: Option<String>,
    password: Option<String>,
}

/// Every session starts as the device's login shell in the chosen folder;
/// AI agents are launched inside it afterwards, from the session header.
/// Returns the command plus a password to auto-type once an `ssh` password
/// prompt appears, for a managed server using password auth.
fn build_command(
    session: &SessionSummary,
    managed: Option<&ManagedConnection>,
) -> Result<(CommandBuilder, Option<String>), String> {
    if session.endpoint == "local" {
        let mut command = CommandBuilder::new(local_shell());
        command.arg("-l");
        command.cwd(&session.project_path);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // Inherited by anything run interactively in this shell, including
        // an AI CLI's own hook scripts, so a hook can report activity back
        // to the fastade session it is actually running inside.
        command.env("FASTADE_SESSION_ID", &session.id);
        return Ok((command, None));
    }

    let mut command = CommandBuilder::new("ssh");
    command.env("TERM", "xterm-256color");
    command.args([
        "-tt",
        "-o",
        &format!("ServerAliveInterval={SSH_KEEPALIVE_INTERVAL_SECONDS}"),
        "-o",
        &format!("ServerAliveCountMax={SSH_KEEPALIVE_FAILURES}"),
        "-o",
        "TCPKeepAlive=yes",
    ]);
    let password = if let Some(server) = managed {
        command.args(["-p", &server.port.to_string()]);
        if let Some(path) = &server.key_path {
            command.args(["-i", path]);
        }
        command.arg(format!("{}@{}", server.username, server.host));
        server.password.clone()
    } else {
        command.arg(&session.endpoint);
        None
    };
    if session.project_path != "~" {
        command.arg(format!(
            "cd -- {} && exec \"${{SHELL:-/bin/sh}}\" -l",
            shell_quote(&session.project_path)
        ));
    }
    Ok((command, password))
}

fn local_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_file())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".to_owned()
            } else {
                "/bin/bash".to_owned()
            }
        })
}

fn validate_input(input: &CreateSessionInput) -> Result<(), String> {
    if input.project_path.trim().is_empty() || input.endpoint.trim().is_empty() {
        return Err("Choose a server and a folder.".to_owned());
    }
    if input.endpoint == "local" && !Path::new(&input.project_path).is_dir() {
        return Err(format!(
            "The chosen folder does not exist: {}",
            input.project_path
        ));
    }
    Ok(())
}

fn get_terminal(
    session_id: &str,
    state: &tauri::State<'_, AppState>,
) -> Result<Arc<TerminalHandle>, String> {
    state
        .terminals
        .lock()
        .map_err(|_| "terminal store is unavailable".to_owned())?
        .get(session_id)
        .cloned()
        .ok_or_else(|| "terminal is not running".to_owned())
}

fn find_session(
    session_id: &str,
    state: &tauri::State<'_, AppState>,
) -> Result<SessionSummary, String> {
    state
        .sessions
        .lock()
        .map_err(|_| "session store is unavailable".to_owned())?
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| "session not found".to_owned())
}

fn read_sessions(path: &Path) -> Result<Vec<SessionSummary>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| format!("session data is invalid: {error}"))
}

fn persist_sessions(state: &AppState) -> Result<(), String> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "session store is unavailable".to_owned())?
        .clone();
    write_sessions(&state.sessions_path, &sessions)
}

fn write_sessions(path: &Path, sessions: &[SessionSummary]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = serde_json::to_string_pretty(sessions).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn record_and_emit(app: &tauri::AppHandle, session_id: &str, content: String) {
    let state = app.state::<AppState>();
    if let Ok(mut transcripts) = state.transcripts.lock() {
        let transcript = transcripts.entry(session_id.to_owned()).or_default();
        if transcript.len() + content.len() > MAX_TRANSCRIPT_BYTES {
            transcript.clear();
            transcript.push_str("\r\n[scrollback truncated]\r\n");
        }
        transcript.push_str(&content);
    }
    emit_event(app, session_id, "output", content);
}

#[derive(Default)]
struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut decoded = String::new();
        let mut consumed = 0;

        while consumed < self.pending.len() {
            match std::str::from_utf8(&self.pending[consumed..]) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    consumed = self.pending.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        decoded.push_str(
                            std::str::from_utf8(&self.pending[consumed..valid_end])
                                .expect("UTF-8 validator reported a valid prefix"),
                        );
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            decoded.push('\u{fffd}');
                            consumed = valid_end + invalid_len;
                        }
                        None => {
                            consumed = valid_end;
                            break;
                        }
                    }
                }
            }
        }

        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        decoded
    }

    fn finish(&mut self) -> String {
        let decoded = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        decoded
    }
}

fn emit_event(app: &tauri::AppHandle, session_id: &str, kind: &'static str, content: String) {
    let _ = app.emit(
        "terminal-event",
        TerminalEvent {
            session_id: session_id.to_owned(),
            kind,
            content,
        },
    );
}

/// Resident memory (bytes) per running session: the session's shell process
/// plus every descendant it has spawned (an AI agent CLI, its own child
/// processes, etc.), so the UI badge reflects what that session is actually
/// costing, not just the idle shell. One `sysinfo` refresh serves every
/// session in a single low-frequency poll from the frontend, so this stays
/// cheap even with several sessions open — see fastade-desktop's memory
/// design notes.
#[tauri::command]
pub fn session_memory_usage(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, u64>, String> {
    collect_session_memory_usage(&state)
}

fn collect_session_memory_usage(state: &AppState) -> Result<HashMap<String, u64>, String> {
    let pids: Vec<(String, sysinfo::Pid)> = state
        .terminals
        .lock()
        .map_err(|_| "terminal store is unavailable".to_owned())?
        .iter()
        .filter_map(|(id, handle)| {
            handle
                .pid
                .map(|pid| (id.clone(), sysinfo::Pid::from_u32(pid)))
        })
        .collect();
    if pids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut children_of: HashMap<sysinfo::Pid, Vec<sysinfo::Pid>> = HashMap::new();
    for (pid, process) in system.processes() {
        if let Some(parent) = process.parent() {
            children_of.entry(parent).or_default().push(*pid);
        }
    }

    let mut usage = HashMap::with_capacity(pids.len());
    for (session_id, root_pid) in pids {
        let mut total = 0_u64;
        let mut stack = vec![root_pid];
        let mut visited = std::collections::HashSet::new();
        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            if let Some(process) = system.process(pid) {
                total += process.memory();
            }
            if let Some(children) = children_of.get(&pid) {
                stack.extend(children);
            }
        }
        usage.insert(session_id, total);
    }
    Ok(usage)
}

/// Polled by the frontend at the same low frequency as `session_memory_usage`.
/// Ground-truth activity for whichever sessions have a CLI hook wired up;
/// sessions without one simply never appear here, and the frontend keeps
/// using its PTY-text heuristic for those.
#[tauri::command]
pub fn session_activity_overrides(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    state
        .activity_overrides
        .lock()
        .map(|value| value.clone())
        .map_err(|_| "activity store is unavailable".to_owned())
}

pub(crate) fn set_activity_override(
    session_id: &str,
    activity: &str,
    state: &tauri::State<'_, AppState>,
) {
    if let Ok(mut overrides) = state.activity_overrides.lock() {
        overrides.insert(session_id.to_owned(), activity.to_owned());
    }
}

/// Called when the frontend stops an agent (■, or the shell exits): a hook's
/// last-reported value would otherwise sit in the store forever (nothing
/// else clears it on a Ctrl-C exit), which could later be mistaken for a
/// fresh "still running" signal.
#[tauri::command]
pub fn clear_session_activity_override(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if let Ok(mut overrides) = state.activity_overrides.lock() {
        overrides.remove(&session_id);
    }
    Ok(())
}

/// A complete, read-only snapshot of everything shown as a saved session in
/// the UI, overlaid with the newest matching live/reconnectable session state.
/// Runtime sessions without a saved profile are appended as well. Hook-backed
/// activity is included when available; CLIs without lifecycle hooks return
/// `null` rather than presenting a heuristic as authoritative MCP data.
pub(crate) fn mcp_sessions(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
) -> Result<Vec<McpSessionSummary>, String> {
    let memory = collect_session_memory_usage(state)?;
    let activity = state
        .activity_overrides
        .lock()
        .map_err(|_| "activity store is unavailable".to_owned())?
        .clone();
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "session store is unavailable".to_owned())?
        .clone();
    let profiles = crate::saved_sessions::list_saved_sessions(app.clone())?;
    Ok(merge_mcp_sessions(profiles, sessions, &memory, &activity))
}

fn merge_mcp_sessions(
    profiles: Vec<crate::saved_sessions::SavedSession>,
    mut sessions: Vec<SessionSummary>,
    memory: &HashMap<String, u64>,
    activity: &HashMap<String, String>,
) -> Vec<McpSessionSummary> {
    let mut result = Vec::with_capacity(profiles.len().max(sessions.len()));
    for profile in profiles {
        let project_path = profile
            .device_paths
            .get(&profile.last_endpoint)
            .cloned()
            .unwrap_or_else(|| "~".to_owned());
        let matching_index = sessions.iter().position(|session| {
            session.endpoint == profile.last_endpoint && session.project_path == project_path
        });
        if let Some(index) = matching_index {
            let session = sessions.remove(index);
            result.push(mcp_summary(
                session,
                Some(profile.id),
                true,
                memory,
                activity,
            ));
        } else {
            result.push(McpSessionSummary {
                id: profile.id.clone(),
                profile_id: Some(profile.id),
                session_id: None,
                saved: true,
                title: profile.name,
                cli: None,
                model: None,
                endpoint: profile.last_endpoint,
                project_path,
                status: McpSessionStatus::Disconnected,
                agent_running: false,
                activity: None,
                memory_bytes: None,
            });
        }
    }
    result.extend(
        sessions
            .into_iter()
            .map(|session| mcp_summary(session, None, false, memory, activity)),
    );
    result
}

fn mcp_summary(
    session: SessionSummary,
    profile_id: Option<String>,
    saved: bool,
    memory: &HashMap<String, u64>,
    activity: &HashMap<String, String>,
) -> McpSessionSummary {
    let agent_running = session.cli.is_some();
    let session_id = session.id.clone();
    McpSessionSummary {
        memory_bytes: memory.get(&session_id).copied(),
        activity: agent_running
            .then(|| activity.get(&session_id).cloned())
            .flatten(),
        agent_running,
        id: session_id.clone(),
        profile_id,
        session_id: Some(session_id),
        saved,
        title: session.title,
        cli: session.cli,
        model: session.model,
        endpoint: session.endpoint,
        project_path: session.project_path,
        status: session.status.into(),
    }
}

/// Resolves a UI-selected session id. Title and folder matching are retained
/// for compatibility with saved profiles created by earlier builds.
fn resolve_session(
    target: &str,
    state: &tauri::State<'_, AppState>,
) -> Result<SessionSummary, String> {
    state
        .sessions
        .lock()
        .map_err(|_| "session store is unavailable".to_owned())?
        .clone()
        .into_iter()
        .find(|session| {
            session.id == target
                || session.title.eq_ignore_ascii_case(target)
                || Path::new(&session.project_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().eq_ignore_ascii_case(target))
                    .unwrap_or(false)
        })
        .ok_or_else(|| format!("no session found matching \"{target}\""))
}

/// Copies `source_path` (resolved by the caller, typically against its own
/// working directory) into the target session's project folder. Does not
/// require the target session's shell to be running — this only touches the
/// filesystem (locally, or over a one-off SSH exec for a remote session's
/// endpoint, using the same stored credentials the interactive launcher
/// uses — the caller never sees them).
fn send_file_to_session(
    target: &str,
    source_path: &str,
    dest_path: Option<&str>,
    state: &tauri::State<'_, AppState>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let session = resolve_session(target, state)?;
    let source = Path::new(source_path);
    if !source.is_file() {
        return Err(format!("source file not found: {source_path}"));
    }
    let file_name = || {
        source
            .file_name()
            .ok_or_else(|| "source_path has no file name".to_owned())
    };
    if session.endpoint != "local" {
        let relative = match dest_path {
            Some(relative) => relative.to_owned(),
            None => file_name()?.to_string_lossy().into_owned(),
        };
        // Mirrors `Path::join`'s behavior for the local branch below: an
        // absolute `dest_path` replaces the project folder entirely instead
        // of nesting under it.
        let remote_path = if relative.starts_with('/') {
            relative
        } else {
            format!("{}/{relative}", session.project_path.trim_end_matches('/'))
        };
        crate::remote_fs::upload_file(app, &session.endpoint, source, &remote_path)?;
        return Ok(remote_path);
    }
    let destination = match dest_path {
        Some(relative) => Path::new(&session.project_path).join(relative),
        None => Path::new(&session.project_path).join(file_name()?),
    };
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, &destination).map_err(|error| error.to_string())?;
    Ok(destination.to_string_lossy().into_owned())
}

/// UI-facing counterpart to [`send_file_to_session`], used by the session
/// status line's upload button: picks up a file the user chose via the
/// native file dialog and drops it at the root of the session's project
/// folder (locally copied, or uploaded over SSH for a remote session).
#[tauri::command]
pub async fn upload_file_to_session(
    session_id: String,
    source_path: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        send_file_to_session(&session_id, &source_path, None, &state, &app)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_home_directory() -> Result<String, String> {
    dirs::home_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .ok_or_else(|| "Could not determine the home directory.".to_owned())
}

pub(crate) fn resolve_program(name: &str) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".local/bin"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::{
        build_command, merge_mcp_sessions, resolve_program, shell_quote, ManagedConnection,
        SessionStatus, SessionSummary, Utf8StreamDecoder,
    };
    use crate::saved_sessions::SavedSession;
    use std::collections::HashMap;

    fn session(endpoint: &str, project_path: &str) -> SessionSummary {
        SessionSummary {
            id: "test".to_owned(),
            title: "test".to_owned(),
            cli: None,
            model: None,
            endpoint: endpoint.to_owned(),
            project_path: project_path.to_owned(),
            status: SessionStatus::Running,
        }
    }

    fn argv(session: &SessionSummary) -> Vec<String> {
        managed_argv(session, None)
    }

    fn managed_argv(session: &SessionSummary, managed: Option<&ManagedConnection>) -> Vec<String> {
        build_command(session, managed)
            .unwrap()
            .0
            .get_argv()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("it's safe"), "'it'\"'\"'s safe'");
    }

    #[test]
    fn mcp_list_includes_saved_profiles_and_overlays_runtime_state() {
        let profiles = vec![
            SavedSession {
                id: "profile-active".to_owned(),
                name: "active project".to_owned(),
                last_endpoint: "local".to_owned(),
                device_paths: HashMap::from([("local".to_owned(), "/active".to_owned())]),
            },
            SavedSession {
                id: "profile-idle".to_owned(),
                name: "idle project".to_owned(),
                last_endpoint: "remote".to_owned(),
                device_paths: HashMap::from([("remote".to_owned(), "/idle".to_owned())]),
            },
        ];
        let sessions = vec![SessionSummary {
            id: "session-active".to_owned(),
            title: "active project".to_owned(),
            cli: None,
            model: None,
            endpoint: "local".to_owned(),
            project_path: "/active".to_owned(),
            status: SessionStatus::Running,
        }];

        let merged = merge_mcp_sessions(profiles, sessions, &HashMap::new(), &HashMap::new());
        let value = serde_json::to_value(merged).unwrap();
        let entries = value.as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["profileId"], "profile-active");
        assert_eq!(entries[0]["sessionId"], "session-active");
        assert_eq!(entries[0]["status"], "running");
        assert_eq!(entries[1]["profileId"], "profile-idle");
        assert!(entries[1]["sessionId"].is_null());
        assert_eq!(entries[1]["status"], "disconnected");
    }

    #[test]
    fn installed_codex_is_discoverable() {
        assert!(resolve_program("codex").is_some());
    }

    #[test]
    fn remote_home_starts_bare_login_shell() {
        assert_eq!(
            argv(&session("kowanas.dev", "~")),
            [
                "ssh",
                "-tt",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "TCPKeepAlive=yes",
                "kowanas.dev"
            ]
        );
    }

    #[test]
    fn remote_folder_starts_login_shell_there() {
        let argv = argv(&session("kowanas.dev", "/root/my app"));
        assert_eq!(
            argv[..9],
            [
                "ssh",
                "-tt",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "TCPKeepAlive=yes",
                "kowanas.dev"
            ]
        );
        assert_eq!(
            argv[9],
            "cd -- '/root/my app' && exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[test]
    fn managed_server_connects_with_port_and_key() {
        let connection = ManagedConnection {
            host: "1.2.3.4".to_owned(),
            port: 2222,
            username: "deploy".to_owned(),
            key_path: Some("/keys/id_ed25519".to_owned()),
            password: None,
        };
        let (command, password) =
            build_command(&session("managed:abc", "~"), Some(&connection)).unwrap();
        let argv: Vec<_> = command
            .get_argv()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            [
                "ssh",
                "-tt",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "TCPKeepAlive=yes",
                "-p",
                "2222",
                "-i",
                "/keys/id_ed25519",
                "deploy@1.2.3.4"
            ]
        );
        assert!(password.is_none());
    }

    #[test]
    fn managed_server_password_is_returned_for_autofill() {
        let connection = ManagedConnection {
            host: "1.2.3.4".to_owned(),
            port: 22,
            username: "root".to_owned(),
            key_path: None,
            password: Some("hunter2".to_owned()),
        };
        let (_, password) = build_command(&session("managed:abc", "~"), Some(&connection)).unwrap();
        assert_eq!(password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn local_session_is_a_login_shell_not_an_agent() {
        let argv = argv(&session("local", "/tmp"));
        assert!(argv[0].ends_with("sh"), "expected a shell, got {argv:?}");
        assert_eq!(argv[1], "-l");
    }

    #[test]
    fn utf8_decoder_preserves_hangul_split_across_reads() {
        let mut decoder = Utf8StreamDecoder::default();
        let bytes = "한글 출력".as_bytes();
        let mut decoded = String::new();
        for byte in bytes {
            decoded.push_str(&decoder.push(&[*byte]));
        }
        decoded.push_str(&decoder.finish());
        assert_eq!(decoded, "한글 출력");
    }
}
