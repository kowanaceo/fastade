use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::session::{most_recent_local_path, resolve_program, AppState};

/// Claude's limits come from driving its TUI, which is slow and fragile, so a
/// good result is reused for this long unless the user presses refresh.
const CLAUDE_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
static CLAUDE_CACHE: Mutex<Option<(Instant, AgentUsage)>> = Mutex::new(None);
const CODEX_LOG_MAX_AGE_SECONDS: i64 = 5 * 60;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    label: String,
    remaining_percent: i64,
    /// Absolute Unix epoch seconds, never milliseconds. Past instants are kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_text: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    agent_id: String,
    status: &'static str,
    windows: Vec<UsageWindow>,
    message: Option<String>,
    /// Provider event time for logs, response receipt time for live queries.
    /// Reading a cached result never changes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    collected_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_email: Option<String>,
}

#[tauri::command]
pub async fn get_agent_usage(
    agent_id: String,
    force: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<AgentUsage, String> {
    let requested_id = agent_id.clone();
    let account = local_account(&requested_id);
    if requested_id == "claude" && !force.unwrap_or(false) {
        if let Some(cached) = fresh_claude_cache(account.as_ref()) {
            return Ok(cached);
        }
    }
    // Claude Code's interactive TUI shows a one-time "do you trust this
    // folder?" prompt the first time it runs in a directory, and it can't be
    // answered by this headless probe. Launch it from wherever the user most
    // recently ran a local session (almost certainly already trusted, since
    // they were using Claude Code there) instead of an unpredictable
    // inherited working directory, which made this fail far more often than
    // the equivalent Codex lookup.
    let claude_cwd = most_recent_local_path(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let mut usage = match requested_id.as_str() {
            "codex" => fetch_codex_usage(force.unwrap_or(false)),
            "claude" => fetch_claude_usage(claude_cwd),
            _ => Ok(AgentUsage {
                agent_id: requested_id.clone(),
                status: "unavailable",
                windows: Vec::new(),
                message: Some("Check the Usage page.".to_owned()),
                collected_at: None,
                account_id: None,
                account_email: None,
            }),
        }?;
        // Never attach a newly signed-in account to limits collected before
        // the account switch. The next refresh will collect that account.
        if account != local_account(&requested_id) {
            return Err("Agent account changed during usage collection; refresh again.".to_owned());
        }
        usage.account_id = account.as_ref().map(|a| a.account_id.clone());
        usage.account_email = account.and_then(|a| a.label);
        if requested_id == "claude" && usage.status == "available" {
            if let Ok(mut cache) = CLAUDE_CACHE.lock() {
                *cache = Some((Instant::now(), usage.clone()));
            }
        }
        Ok(usage)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn fresh_claude_cache(account: Option<&AgentAccount>) -> Option<AgentUsage> {
    let cache = CLAUDE_CACHE.lock().ok()?;
    let (at, usage) = cache.as_ref()?;
    let reset = usage
        .windows
        .iter()
        .any(|window| window.resets_at.is_some_and(|at| at <= now_unix()));
    (at.elapsed() < CLAUDE_CACHE_TTL
        && usage.account_id.as_deref() == account.map(|a| a.account_id.as_str())
        && usage.account_email.as_deref() == account.and_then(|a| a.label.as_deref())
        && !reset)
        .then(|| usage.clone())
}

/// Logs are a short-lived cache. A manual refresh always asks the account,
/// including when this device has not run a new Codex turn recently.
fn fetch_codex_usage(force: bool) -> Result<AgentUsage, String> {
    codex_usage_with_refresh(
        &codex_sessions_dir(),
        force,
        now_unix(),
        fetch_codex_usage_from_app_server,
    )
}

fn codex_usage_with_refresh(
    root: &Path,
    force: bool,
    now: i64,
    fetch_live: impl FnOnce() -> Result<AgentUsage, String>,
) -> Result<AgentUsage, String> {
    if !force {
        if let Some((windows, written)) = codex_log_usage(root) {
            // A missing window may have reset since the log was written.
            if (0..CODEX_LOG_MAX_AGE_SECONDS).contains(&(now - written))
                && windows.len() == 2
                && !windows
                    .iter()
                    .any(|window| window.resets_at.is_some_and(|at| at <= now))
            {
                return Ok(AgentUsage {
                    agent_id: "codex".to_owned(),
                    status: "available",
                    windows,
                    message: None,
                    collected_at: Some(written),
                    account_id: None,
                    account_email: None,
                });
            }
        }
    }
    fetch_live()
}

fn codex_sessions_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_default()
        .join("sessions")
}

/// Most recently modified `rollout-*.jsonl` files first. An older session
/// can still be active after a newer one has stopped writing.
fn newest_rollouts(root: &Path, limit: usize) -> Vec<PathBuf> {
    fn descending(path: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|dir| dir.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        entries.reverse();
        entries
    }
    let mut files = Vec::new();
    for year in descending(root) {
        for month in descending(&year) {
            for day in descending(&month) {
                for file in descending(&day) {
                    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                        files.push(file);
                    }
                }
            }
        }
    }
    files.sort_by_cached_key(|path| {
        std::cmp::Reverse((
            path.metadata().ok().and_then(|meta| meta.modified().ok()),
            path.clone(),
        ))
    });
    files.truncate(limit);
    files
}

fn read_tail(path: &Path, bytes: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(bytes)))
        .ok()?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).ok()?;
    Some(String::from_utf8_lossy(&contents).into_owned())
}

fn codex_log_usage(root: &Path) -> Option<(Vec<UsageWindow>, i64)> {
    newest_rollouts(root, 8)
        .iter()
        .filter_map(|path| read_tail(path, 256 * 1024))
        .filter_map(|tail| codex_usage_from_log(&tail))
        .max_by_key(|(_, written)| *written)
}

/// The last `token_count` event's rate limits in `text` (a log tail, or the
/// one line a remote host sends back), as windows plus when it was written.
/// Expired reset instants are preserved so consumers can identify stale limits.
fn codex_usage_from_log(text: &str) -> Option<(Vec<UsageWindow>, i64)> {
    let event = text.lines().rev().find_map(|line| {
        if !line.contains("\"rate_limits\"") {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        (value.pointer("/payload/type")?.as_str()? == "token_count"
            && value.pointer("/payload/rate_limits/primary").is_some())
        .then_some(value)
    })?;
    let written = written_at(&event)?;
    let limits = event.pointer("/payload/rate_limits")?;
    let mut windows = Vec::new();
    for (label, key) in [("5h", "primary"), ("Week", "secondary")] {
        let value = &limits[key];
        let used = value.get("used_percent").and_then(Value::as_f64);
        let Some(used) = used else {
            continue;
        };
        let (resets_at, reset_text) = reset_fields(value, "resets_at", "reset_text", written);
        windows.push(UsageWindow {
            label: label.to_owned(),
            remaining_percent: (100.0 - used).round().clamp(0.0, 100.0) as i64,
            resets_at,
            reset_text,
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some((windows, written))
}

fn written_at(event: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(event.get("timestamp")?.as_str()?)
        .ok()
        .map(|at| at.timestamp())
}

fn fetch_codex_usage_from_app_server() -> Result<AgentUsage, String> {
    let program =
        resolve_program("codex").ok_or_else(|| "Could not find the Codex CLI.".to_owned())?;
    let mut child = Command::new(program)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Failed to start the Codex usage collector: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Could not open Codex usage input.".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not open Codex usage output.".to_owned())?;
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"fastade","version":"0.1.0"},"capabilities":{"experimentalApi":true}}}),
        json!({"jsonrpc":"2.0","method":"initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"account/rateLimits/read","params":{"excludeResetCreditDetails":true}}),
    ] {
        writeln!(stdin, "{message}").map_err(|error| error.to_string())?;
    }
    stdin.flush().map_err(|error| error.to_string())?;

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = BufReader::new(stdout)
            .lines()
            .filter_map(Result::ok)
            .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
            .find(|message| message.get("id").and_then(Value::as_i64) == Some(2));
        let _ = sender.send(result);
    });
    let received = receiver.recv_timeout(Duration::from_secs(12));
    let collected_at = now_unix();
    let _ = child.kill();
    let _ = child.wait();
    let result = received.map_err(|_| "Codex usage lookup timed out.".to_owned())?;
    let result = result
        .and_then(|message| message.get("result").cloned())
        .ok_or_else(|| "Did not receive a Codex usage response.".to_owned())?;
    let limits = &result["rateLimits"];
    let mut windows = Vec::new();
    append_window(&mut windows, "5h", &limits["primary"], collected_at);
    append_window(&mut windows, "Week", &limits["secondary"], collected_at);
    Ok(AgentUsage {
        agent_id: "codex".to_owned(),
        status: if windows.is_empty() {
            "unavailable"
        } else {
            "available"
        },
        windows,
        message: None,
        collected_at: Some(collected_at),
        account_id: None,
        account_email: None,
    })
}

fn append_window(windows: &mut Vec<UsageWindow>, label: &str, value: &Value, collected_at: i64) {
    let Some(used_percent) = value.get("usedPercent").and_then(Value::as_f64) else {
        return;
    };
    let (resets_at, reset_text) = reset_fields(value, "resetsAt", "resetText", collected_at);
    windows.push(UsageWindow {
        label: label.to_owned(),
        remaining_percent: (100.0 - used_percent).round().clamp(0.0, 100.0) as i64,
        resets_at,
        reset_text,
    });
}

fn reset_fields(
    value: &Value,
    instant_key: &str,
    text_key: &str,
    collected_at: i64,
) -> (Option<i64>, Option<String>) {
    let text = value
        .get(text_key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty());
    let instant = value
        .get(instant_key)
        .and_then(Value::as_f64)
        .filter(|at| at.is_finite() && *at >= 0.0 && *at < i64::MAX as f64)
        // Be defensive about millisecond timestamps from a provider adapter.
        .map(|at| {
            (if at >= 1_000_000_000_000.0 {
                at / 1000.0
            } else {
                at
            })
            .floor() as i64
        })
        .or_else(|| text.and_then(|text| parse_reset_instant(text, collected_at)));
    (
        instant,
        if instant.is_some() {
            None
        } else {
            text.map(str::to_owned)
        },
    )
}

fn fetch_claude_usage(cwd: Option<String>) -> Result<AgentUsage, String> {
    let program =
        resolve_program("claude").ok_or_else(|| "Could not find the Claude CLI.".to_owned())?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Could not open a PTY for Claude usage: {error}"))?;
    let mut command = CommandBuilder::new(program);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let work_dir = cwd
        .filter(|path| is_trusted_claude_directory(path))
        .or_else(trusted_claude_directory)
        .or_else(|| dirs::home_dir().map(|path| path.to_string_lossy().into_owned()));
    if let Some(dir) = work_dir {
        command.cwd(dir);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start the Claude usage collector: {error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Could not open Claude usage output: {error}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Could not open Claude usage input: {error}"))?;
    drop(pair.slave);

    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let captured = output.clone();
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            if let Ok(mut bytes) = captured.lock() {
                if bytes.len() < 512 * 1024 {
                    bytes.extend_from_slice(&buffer[..count]);
                }
            }
        }
    });

    // Claude Code's startup time is not fixed (it can fall back to a "classic"
    // renderer when the fullscreen one didn't finish starting previously), so a
    // short fixed sleep before typing can race the CLI's input handling and
    // silently drop the command. Wait for the prompt box to actually render,
    // or for the one-time "do you trust this folder?" prompt, which this
    // headless probe can't answer on the user's behalf.
    wait_for_output(&output, Duration::from_millis(6_000), |text| {
        text.contains("auto mode on") || text.contains("/effort") || is_trust_prompt(text)
    });

    if is_trust_prompt(&current_output(&output)) {
        let _ = child.kill();
        let _ = child.wait();
        drop(writer);
        let _ = reader_thread.join();
        return Ok(AgentUsage {
            agent_id: "claude".to_owned(),
            status: "unavailable",
            windows: Vec::new(),
            message: Some(
                "Claude Code has not trusted this folder yet. Run claude once in a terminal to complete the trust prompt, then try again.".to_owned(),
            ),
            collected_at: None,
            account_id: None,
            account_email: None,
        });
    }

    // Typing the command and pressing Enter in the same write can also race the
    // slash-command autocomplete: split them with a short pause, like a human
    // typing, so Enter lands after the command is recognized.
    if let Err(error) = writer.write_all(b"/usage").and_then(|_| writer.flush()) {
        let _ = child.kill();
        let _ = child.wait();
        drop(writer);
        let _ = reader_thread.join();
        return Err(format!("Failed to send the Claude /usage command: {error}"));
    }
    thread::sleep(Duration::from_millis(200));
    if let Err(error) = writer.write_all(b"\r").and_then(|_| writer.flush()) {
        let _ = child.kill();
        let _ = child.wait();
        drop(writer);
        let _ = reader_thread.join();
        return Err(format!("Failed to send the Claude /usage command: {error}"));
    }

    // Wait for the usage panel to finish loading instead of a fixed sleep: the
    // percentage bars stream in after a brief "Loading usage data…" state. The
    // panel always shows a "Total cost:" line immediately, before the numbers
    // we actually need, so that alone is not a reliable completion signal.
    wait_for_output(&output, Duration::from_millis(7_000), |text| {
        text.contains("% used") || text.to_ascii_lowercase().contains("api usage billing")
    });
    // The second window's percentage can land a beat after the first one does.
    thread::sleep(Duration::from_millis(300));
    let collected_at = now_unix();

    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    let _ = reader_thread.join();

    let bytes = output
        .lock()
        .map_err(|_| "Could not read Claude usage output.".to_owned())?;
    let text = terminal_text(&String::from_utf8_lossy(&bytes));
    if std::env::var("FASTADE_DEBUG_USAGE").is_ok() {
        eprintln!("RAW CLAUDE USAGE TEXT:\n{text}");
    }
    Ok(parse_claude_usage_at(&text, collected_at))
}

fn current_output(output: &Arc<Mutex<Vec<u8>>>) -> String {
    let bytes = match output.lock() {
        Ok(bytes) => bytes,
        Err(poisoned) => poisoned.into_inner(),
    };
    terminal_text(&String::from_utf8_lossy(&bytes))
}

fn is_trust_prompt(text: &str) -> bool {
    text.to_ascii_lowercase().contains("trust this folder")
}

/// A directory the user has already told Claude Code (via its own trust
/// prompt) that it can use, read from `~/.claude.json`'s `projects` map. Used
/// so a fresh fastade install (with no local session yet, so no better
/// candidate) doesn't have to guess a directory and gets stuck on the trust
/// prompt on the very first usage check. Read-only: this never grants trust
/// itself, only reuses a decision the user already made.
fn trusted_claude_directory() -> Option<String> {
    claude_trust_map()?
        .into_iter()
        .find_map(|(dir, trusted)| (trusted && std::path::Path::new(&dir).is_dir()).then_some(dir))
}

/// Whether Claude Code has already recorded this exact directory as trusted
/// (via `~/.claude.json`'s `hasTrustDialogAccepted`). A directory merely
/// existing isn't enough: the most-recent local session's project path can
/// be one the user never ran `claude` in directly (e.g. a subdirectory of a
/// trusted repo), which still trips the interactive trust prompt.
fn is_trusted_claude_directory(path: &str) -> bool {
    std::path::Path::new(path).is_dir()
        && claude_trust_map()
            .and_then(|map| map.into_iter().find(|(dir, _)| dir == path))
            .is_some_and(|(_, trusted)| trusted)
}

fn claude_trust_map() -> Option<Vec<(String, bool)>> {
    let path = dirs::home_dir()?.join(".claude.json");
    let contents = std::fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&contents).ok()?;
    let projects = json.get("projects")?.as_object()?;
    Some(
        projects
            .iter()
            .map(|(dir, meta)| {
                let trusted = meta
                    .get("hasTrustDialogAccepted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (dir.clone(), trusted)
            })
            .collect(),
    )
}

/// Polls the captured PTY output until `ready` matches or `timeout` elapses.
fn wait_for_output(
    output: &Arc<Mutex<Vec<u8>>>,
    timeout: Duration,
    ready: impl Fn(&str) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let text = current_output(output);
        if ready(&text) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(120));
    }
}

#[cfg(test)]
fn parse_claude_usage(text: &str) -> AgentUsage {
    parse_claude_usage_at(text, now_unix())
}

fn parse_claude_usage_at(text: &str, collected_at: i64) -> AgentUsage {
    let mut windows = Vec::new();
    for (label, headings) in [
        ("5h", &["current session", "5-hour", "5 hour"][..]),
        (
            "Week",
            &["current week (all models)", "all models", "weekly limit"][..],
        ),
        ("Sonnet", &["current week (sonnet only)", "sonnet only"][..]),
    ] {
        if let Some(section) = usage_section(text, headings) {
            if let Some((percent, used)) = find_percent(section) {
                let reset_text = reset_description(section);
                // Normalize the printed reset into epoch seconds at collection
                // time. Preserve the full description if it cannot be resolved.
                let resets_at = reset_text
                    .as_deref()
                    .and_then(|text| parse_reset_instant(text, collected_at));
                windows.push(UsageWindow {
                    label: label.to_owned(),
                    remaining_percent: if used { 100 - percent } else { percent }.clamp(0, 100),
                    resets_at,
                    reset_text: if resets_at.is_some() {
                        None
                    } else {
                        reset_text
                    },
                });
            }
        }
    }
    windows.dedup_by(|left, right| left.label == right.label);
    let lower = text.to_ascii_lowercase();
    let message = if windows.is_empty()
        && (lower.contains("api usage billing") || lower.contains("total cost"))
    {
        Some("API billing account · no subscription limit".to_owned())
    } else if windows.is_empty() {
        Some("Could not find limit info in Claude /usage.".to_owned())
    } else {
        None
    };
    AgentUsage {
        agent_id: "claude".to_owned(),
        status: if windows.is_empty() {
            "unavailable"
        } else {
            "available"
        },
        windows,
        message,
        collected_at: Some(collected_at),
        account_id: None,
        account_email: None,
    }
}

fn usage_section<'a>(text: &'a str, headings: &[&str]) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let start = headings
        .iter()
        .filter_map(|heading| lower.rfind(heading).map(|index| index + heading.len()))
        .max()?;
    let tail = &text[start..];
    let tail_lower = tail.to_ascii_lowercase();
    let end = [
        "current session",
        "current week",
        "5-hour",
        "5 hour",
        "weekly limit",
    ]
    .iter()
    .filter_map(|heading| tail_lower.find(heading))
    .min()
    .unwrap_or(tail.len());
    Some(&tail[..end])
}

fn find_percent(section: &str) -> Option<(i64, bool)> {
    let bytes = section.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut start = index;
        while start > 0 && bytes[start - 1].is_ascii_whitespace() {
            start -= 1;
        }
        let end = start;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let percent = section[start..end].parse::<i64>().ok()?;
        let context_end = (index + 24).min(section.len());
        let context = section[start..context_end].to_ascii_lowercase();
        return Some((percent.clamp(0, 100), context.contains("used")));
    }
    None
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Turns a Claude Code reset phrase into a Unix timestamp: either a
/// relative duration from the collection instant, or a clock/date in the
/// printed IANA timezone (system timezone only when none was printed).
fn parse_reset_instant(text: &str, collected_at: i64) -> Option<i64> {
    if let Some(seconds) = parse_relative_duration(text) {
        return collected_at.checked_add(seconds);
    }
    if let Some((_, zone)) = text.rsplit_once('(') {
        let zone: chrono_tz::Tz = zone.trim().strip_suffix(')')?.trim().parse().ok()?;
        parse_absolute_reset(text, collected_at, zone)
    } else {
        parse_absolute_reset(text, collected_at, chrono::Local)
    }
}

fn parse_absolute_reset<T: chrono::TimeZone>(
    text: &str,
    collected_at: i64,
    zone: T,
) -> Option<i64> {
    use chrono::{Datelike, NaiveTime};

    let without_tz = text.split('(').next().unwrap_or(text).trim();
    let lower = without_tz.to_ascii_lowercase();
    let after_resets = lower.find("resets")? + "resets".len();
    let rest = lower[after_resets..].trim();
    if rest.is_empty() {
        return None;
    }

    let (date_part, time_part) = match rest.split_once(" at ") {
        Some((date, time)) => (Some(date.trim()), Some(time.trim())),
        None if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) => (None, Some(rest)),
        None => (Some(rest), None),
    };

    let now = zone.timestamp_opt(collected_at, 0).single()?;
    let date = match date_part {
        Some(date_text) => parse_month_day(date_text, now.year())?,
        None => now.date_naive(),
    };
    let time = match time_part {
        Some(time_text) => parse_clock_time(time_text)?,
        None => NaiveTime::from_hms_opt(0, 0, 0)?,
    };

    // Without a date use the collection date in that timezone. Do not roll
    // an elapsed reset into tomorrow or a later cycle. Ambiguous/nonexistent
    // DST times fall back to the original text rather than guessing.
    Some(
        zone.from_local_datetime(&date.and_time(time))
            .single()?
            .timestamp(),
    )
}

fn parse_clock_time(text: &str) -> Option<chrono::NaiveTime> {
    let mut normalized = text.trim().to_ascii_uppercase().replace(' ', "");
    if !normalized.contains(':') && (normalized.ends_with("AM") || normalized.ends_with("PM")) {
        let split_at = normalized.len().checked_sub(2)?;
        normalized.insert_str(split_at, ":00");
    }
    chrono::NaiveTime::parse_from_str(&normalized, "%I:%M%p")
        .ok()
        .or_else(|| chrono::NaiveTime::parse_from_str(&normalized, "%H:%M").ok())
}

fn parse_month_day(text: &str, year: i32) -> Option<chrono::NaiveDate> {
    let text = text.trim();
    ["%b %d %Y", "%B %d %Y"].into_iter().find_map(|format| {
        chrono::NaiveDate::parse_from_str(text, format)
            .ok()
            .or_else(|| chrono::NaiveDate::parse_from_str(&format!("{text} {year}"), format).ok())
    })
}

/// Parses a relative reset phrase like "in 3h 12m", "in 45m", "in 2d 3h"
/// into a second count, by scanning "<number><unit>" tokens (d/h/m/s) after
/// the word "in". Returns `None` for anything else (an absolute date/time).
fn parse_relative_duration(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let description = lower.split('(').next()?.trim();
    let mut rest = description
        .strip_prefix("resets ")
        .unwrap_or(description)
        .strip_prefix("in ")?
        .trim();
    let mut seconds = 0_i64;
    let mut matched = false;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let number = rest[..digits].parse::<i64>().ok()?;
        rest = rest[digits..].trim_start();
        let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let unit_seconds = match &rest[..letters] {
            "d" | "day" | "days" => 86_400,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            _ => return None,
        };
        seconds = seconds.checked_add(number.checked_mul(unit_seconds)?)?;
        matched = true;
        rest = rest[letters..].trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        rest = rest.strip_prefix("and ").unwrap_or(rest).trim_start();
    }
    matched.then_some(seconds)
}

fn reset_description(section: &str) -> Option<String> {
    let lower = section.to_ascii_lowercase();
    let start = lower.find("reset")?;
    let value = section[start..]
        .split(['\n', '│', '┃'])
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!value.is_empty()).then_some(value)
}

fn terminal_text(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            if bytes.get(index + 1) == Some(&b']') {
                index += 2;
                while index < bytes.len() && bytes[index] != 0x07 {
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            } else if bytes.get(index + 1) == Some(&b'[') {
                index += 2;
                while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
                    index += 1;
                }
                output.push(' ');
            } else {
                index += 1;
            }
        } else if bytes[index] == b'\r' || bytes[index] == b'\n' {
            output.push('\n');
        } else if bytes[index] >= 0x20 {
            let rest = &value[index..];
            let character = rest.chars().next().unwrap_or('\u{fffd}');
            output.push(character);
            index += character.len_utf8() - 1;
        }
        index += 1;
    }
    output
}

/// One host's limits as of `collected_at`, the unit the account stores so
/// another device (or this one, later) can show them.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    agent_id: String,
    host_id: String,
    host_label: String,
    /// The provider account the limits belong to; hosts signed in to the same
    /// account share one set of limits, so this is what groups them.
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_email: Option<String>,
    collected_at: i64,
    windows: Vec<UsageWindow>,
}

/// Who an agent is signed in to on this device: a stable id plus something
/// readable (the email) when the provider's files hold one.
#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccount {
    agent_id: String,
    account_id: String,
    label: Option<String>,
}

#[cfg(test)]
fn local_account_id(agent_id: &str) -> Option<String> {
    local_account(agent_id).map(|account| account.account_id)
}

fn local_account(agent_id: &str) -> Option<AgentAccount> {
    match agent_id {
        "codex" => codex_account(),
        "claude" => claude_account(),
        _ => None,
    }
}

fn codex_account() -> Option<AgentAccount> {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?
        .join("auth.json");
    let json: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    codex_account_from_json(&json)
}

fn identity_text(value: Option<&Value>, limit: usize) -> Option<String> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.chars().count() <= limit)
        .map(str::to_owned)
}

fn codex_account_from_json(json: &Value) -> Option<AgentAccount> {
    let tokens = json.get("tokens")?;
    let account_id = identity_text(tokens.get("account_id"), 200)?;
    let label = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(|jwt| jwt.split('.').nth(1))
        .and_then(|part| {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part.trim_end_matches('='))
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|claims| identity_text(claims.get("email"), 320));
    Some(AgentAccount {
        agent_id: "codex".to_owned(),
        account_id,
        label,
    })
}

fn claude_account() -> Option<AgentAccount> {
    let path = dirs::home_dir()?.join(".claude.json");
    let json: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    claude_account_from_json(&json)
}

fn claude_account_from_json(json: &Value) -> Option<AgentAccount> {
    let account = json.get("oauthAccount")?;
    Some(AgentAccount {
        agent_id: "claude".to_owned(),
        account_id: identity_text(account.get("accountUuid"), 200)?,
        label: identity_text(account.get("emailAddress"), 320),
    })
}

#[tauri::command]
pub fn get_local_accounts() -> Vec<AgentAccount> {
    [codex_account(), claude_account()]
        .into_iter()
        .flatten()
        .collect()
}

/// Prints the newest Codex limits line on a host. A unix socket is not needed:
/// the desktop app runs this over its ordinary ssh exec, so the host stores
/// nothing and holds no credential.
const REMOTE_ACCOUNT_PYTHON: &str = include_str!("remote/usage_account.py");
const REMOTE_CLAUDE_USAGE_PYTHON: &str = include_str!("remote/claude_usage.py");
const REMOTE_CODEX_LOG_SCRIPT: &str = r#"h="${CODEX_HOME:-$HOME/.codex}"; d="$h/sessions"; [ -d "$d" ] || exit 0; find "$d" -name 'rollout-*.jsonl' -type f 2>/dev/null | sort -r | head -8 | while IFS= read -r f; do l=$(tail -c 262144 "$f" | grep '"rate_limits"' | tail -1); if [ -n "$l" ]; then printf '%s\n' "$l"; break; fi; done"#;

fn remote_account_probe() -> String {
    remote_account_probe_for("codex")
}

fn remote_account_probe_for(agent: &str) -> String {
    if agent == "claude" {
        return format!(
            "if command -v python3 >/dev/null 2>&1; then python3 -c {} claude 2>/dev/null; fi",
            crate::session::shell_quote(REMOTE_ACCOUNT_PYTHON),
        );
    }
    format!(
        "if command -v python3 >/dev/null 2>&1; then python3 -c {} codex 2>/dev/null; else {}; fi",
        crate::session::shell_quote(REMOTE_ACCOUNT_PYTHON),
        r#"grep -o '"account_id" *: *"[^"]*"' "${CODEX_HOME:-$HOME/.codex}/auth.json" 2>/dev/null | head -1 | sed 's/.*: *"\(.*\)"/ACCOUNT \1/'"#,
    )
}

fn remote_account(output: &str) -> Option<AgentAccount> {
    if let Some(encoded) = output
        .lines()
        .find_map(|line| line.strip_prefix("ACCOUNT_JSON "))
    {
        let value: Value = serde_json::from_str(encoded).ok()?;
        let account_id = value.get("accountId")?.as_str()?.trim();
        if account_id.is_empty() || account_id.len() > 200 {
            return None;
        }
        let label = value
            .get("accountEmail")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| !email.is_empty() && email.len() <= 320)
            .map(str::to_owned);
        return Some(AgentAccount {
            agent_id: "codex".to_owned(),
            account_id: account_id.to_owned(),
            label,
        });
    }
    output
        .lines()
        .find_map(|line| line.strip_prefix("ACCOUNT "))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| AgentAccount {
            agent_id: "codex".to_owned(),
            account_id: id.to_owned(),
            label: None,
        })
}

/// Stable, URL-safe id for a host the account keys snapshots by.
pub(crate) fn remote_host_id(endpoint: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(endpoint.as_bytes());
    let hex: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("ssh-{hex}")
}

fn remote_host_label(app: &tauri::AppHandle, endpoint: &str) -> String {
    endpoint
        .strip_prefix("managed:")
        .and_then(|id| crate::servers::resolve_managed_server(app, id).ok())
        .map(|(server, _)| server.host)
        .unwrap_or_else(|| endpoint.to_owned())
}

static REMOTE_CLAUDE_CACHE: Mutex<Vec<(String, Instant, UsageSnapshot)>> = Mutex::new(Vec::new());

/// Codex limits (from its logs) and Claude limits (from `/usage` run in a
/// PTY on the host) for every SSH host with a live session. Claude's limits
/// are not in its logs, so the host needs python3 and a signed-in `claude`.
#[tauri::command]
pub async fn get_remote_usage(
    force: Option<bool>,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<UsageSnapshot>, String> {
    let endpoints = crate::session::running_remote_endpoints(&state);
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let workers: Vec<_> = endpoints
            .into_iter()
            .flat_map(|endpoint| {
                let codex = {
                    let (endpoint, app) = (endpoint.clone(), app.clone());
                    thread::spawn(move || remote_codex_snapshot(&endpoint, &app))
                };
                let claude = {
                    let app = app.clone();
                    thread::spawn(move || remote_claude_snapshot(&endpoint, &app, force))
                };
                [codex, claude]
            })
            .collect();
        Ok(workers
            .into_iter()
            .filter_map(|worker| worker.join().ok().flatten())
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn remote_codex_snapshot(endpoint: &str, app: &tauri::AppHandle) -> Option<UsageSnapshot> {
    let probe = remote_account_probe();
    let script = format!("{probe}; {REMOTE_CODEX_LOG_SCRIPT}");
    let output = crate::remote_fs::run_remote_script(endpoint, &script, app).ok()?;
    let account = remote_account(&output);
    let after = crate::remote_fs::run_remote_script(endpoint, &probe, app).ok()?;
    if account != remote_account(&after) {
        return None;
    }
    let (windows, collected_at) = codex_usage_from_log(&output)?;
    Some(UsageSnapshot {
        agent_id: "codex".to_owned(),
        host_id: remote_host_id(endpoint),
        host_label: remote_host_label(app, endpoint),
        account_id: account.as_ref().map(|a| a.account_id.clone()),
        account_email: account.and_then(|a| a.label),
        collected_at,
        windows,
    })
}

fn remote_claude_snapshot(
    endpoint: &str,
    app: &tauri::AppHandle,
    force: bool,
) -> Option<UsageSnapshot> {
    if !force {
        let cache = REMOTE_CLAUDE_CACHE.lock().ok()?;
        let cached = cache.iter().find(|(key, _, _)| key == endpoint);
        if let Some((_, at, snapshot)) = cached {
            let reset = snapshot
                .windows
                .iter()
                .any(|window| window.resets_at.is_some_and(|at| at <= now_unix()));
            if at.elapsed() < CLAUDE_CACHE_TTL && !reset {
                return Some(snapshot.clone());
            }
        }
    }
    let probe = remote_account_probe_for("claude");
    let script = format!(
        "{probe}; if command -v python3 >/dev/null 2>&1; then python3 -c {}; fi",
        crate::session::shell_quote(REMOTE_CLAUDE_USAGE_PYTHON),
    );
    let output = crate::remote_fs::run_remote_script(endpoint, &script, app).ok()?;
    let collected_at = now_unix();
    let account = remote_account(&output)?;
    let after = crate::remote_fs::run_remote_script(endpoint, &probe, app).ok()?;
    if Some(&account) != remote_account(&after).as_ref() {
        return None;
    }
    let encoded = output
        .lines()
        .find_map(|line| line.strip_prefix("CLAUDE_USAGE_B64 "))?;
    let bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded.trim()).ok()?;
    let text = terminal_text(&String::from_utf8_lossy(&bytes));
    let usage = parse_claude_usage_at(&text, collected_at);
    if usage.windows.is_empty() {
        return None;
    }
    let snapshot = UsageSnapshot {
        agent_id: "claude".to_owned(),
        host_id: remote_host_id(endpoint),
        host_label: remote_host_label(app, endpoint),
        account_id: Some(account.account_id),
        account_email: account.label,
        collected_at,
        windows: usage.windows,
    };
    if let Ok(mut cache) = REMOTE_CLAUDE_CACHE.lock() {
        cache.retain(|(key, _, _)| key != endpoint);
        cache.push((endpoint.to_owned(), Instant::now(), snapshot.clone()));
    }
    Some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::{
        codex_usage_from_log, fetch_claude_usage, newest_rollouts, parse_claude_usage,
        parse_relative_duration, parse_reset_instant, remote_host_id, terminal_text,
    };

    #[test]
    fn provider_account_metadata_does_not_serialize_credentials() {
        use base64::Engine;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"codex@example.com"}"#);
        let auth = serde_json::json!({"tokens": {
            "account_id": "codex-id", "id_token": format!("header.{claims}.signature"),
            "refresh_token": "secret-refresh", "access_token": "secret-access"
        }});
        let account = super::codex_account_from_json(&auth).unwrap();
        assert_eq!(account.account_id, "codex-id");
        assert_eq!(account.label.as_deref(), Some("codex@example.com"));
        let claude = super::claude_account_from_json(&serde_json::json!({
            "emailAddress": "wrong@example.com",
            "oauthAccount": {"accountUuid": "claude-id", "emailAddress": "claude@example.com"}
        }))
        .unwrap();
        assert_eq!(claude.label.as_deref(), Some("claude@example.com"));
        let wire = serde_json::to_string(&account).unwrap();
        assert!(!wire.contains("secret-"));
        assert!(!wire.contains("id_token"));
        let without_email = super::codex_account_from_json(&serde_json::json!({
            "tokens": {"account_id": "codex-id", "id_token": "broken"}
        }))
        .unwrap();
        assert!(without_email.label.is_none());
    }

    #[test]
    fn remote_account_and_snapshot_keep_id_and_email_together() {
        let account = super::remote_account(
            "shell banner\nACCOUNT_JSON {\"accountId\":\"account-a\",\"accountEmail\":\"a@example.com\"}\n"
        ).unwrap();
        let snapshot = super::UsageSnapshot {
            agent_id: "codex".to_owned(),
            host_id: "ssh-test".to_owned(),
            host_label: "Test".to_owned(),
            account_id: Some(account.account_id),
            account_email: account.label,
            collected_at: 123,
            windows: Vec::new(),
        };
        let payload = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(payload["accountId"], "account-a");
        assert_eq!(payload["accountEmail"], "a@example.com");
        assert_eq!(payload["collectedAt"], 123);
        assert!(super::remote_account("ACCOUNT_JSON {}\n").is_none());
        let legacy = super::remote_account("ACCOUNT legacy-id\n").unwrap();
        assert_eq!(legacy.account_id, "legacy-id");
        assert!(legacy.label.is_none());
        let no_email = super::UsageSnapshot {
            account_email: None,
            ..snapshot
        };
        assert!(serde_json::to_value(no_email)
            .unwrap()
            .get("accountEmail")
            .is_none());
    }

    const LOG: &str = concat!(
        r#"{"timestamp":"2026-10-04T13:00:00.000Z","type":"event_msg","payload":{"type":"agent_message"}}"#,
        "\n",
        r#"{"timestamp":"2026-10-04T13:10:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":10.0,"window_minutes":300,"resets_at":2000},"secondary":{"used_percent":2.0,"window_minutes":10080,"resets_at":9000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-10-04T13:14:30.903Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":28.0,"window_minutes":300,"resets_at":2000},"secondary":{"used_percent":4.4,"window_minutes":10080,"resets_at":9000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-10-04T13:15:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        "\n",
    );

    #[test]
    fn codex_limits_come_from_the_last_token_count_with_rate_limits() {
        let (windows, written) = codex_usage_from_log(LOG).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].remaining_percent, 72);
        assert_eq!(windows[0].resets_at, Some(2000));
        assert_eq!(windows[1].label, "Week");
        assert_eq!(windows[1].remaining_percent, 96);
        assert_eq!(written, 1_791_119_670);
    }

    #[test]
    fn expired_windows_keep_their_original_instants() {
        let (windows, _) = codex_usage_from_log(LOG).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].resets_at, Some(2000));
        assert_eq!(windows[1].resets_at, Some(9000));
        assert!(codex_usage_from_log("not json\n").is_none());
    }

    #[test]
    fn rollouts_are_found_newest_first_across_date_folders() {
        let root = std::env::temp_dir().join(format!("fastade-codex-{}", uuid::Uuid::new_v4()));
        for (day, name) in [
            ("2026/09/30", "rollout-2026-09-30T10-00-00-a.jsonl"),
            ("2026/10/04", "rollout-2026-10-04T08-00-00-b.jsonl"),
            ("2026/10/04", "rollout-2026-10-04T21-00-00-c.jsonl"),
            ("2026/10/04", "notes.txt"),
        ] {
            let dir = root.join(day);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, "").unwrap();
            let seconds = if name.contains("21-00") {
                3
            } else if name.contains("08-00") {
                2
            } else {
                1
            };
            std::fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
                    ),
                )
                .unwrap();
        }
        let names: Vec<String> = newest_rollouts(&root, 2)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "rollout-2026-10-04T21-00-00-c.jsonl",
                "rollout-2026-10-04T08-00-00-b.jsonl"
            ]
        );
        assert_eq!(newest_rollouts(&root, 10).len(), 3);
        let _ = std::fs::remove_dir_all(root);
    }

    struct LogFixture(std::path::PathBuf);

    impl LogFixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("fastade-usage-{}", uuid::Uuid::new_v4())))
        }

        fn write(&self, day: &str, written: i64, used: i64, modified: u64) {
            let dir = self.0.join(format!("2026/10/{day}"));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("rollout-{day}.jsonl"));
            let event = serde_json::json!({
                "timestamp": chrono::DateTime::from_timestamp(written, 0).unwrap().to_rfc3339(),
                "payload": {"type": "token_count", "rate_limits": {
                    "primary": {"used_percent": used, "resets_at": 2000},
                    "secondary": {"used_percent": used, "resets_at": 9000}
                }}
            });
            std::fs::write(&path, event.to_string()).unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified),
                    ),
                )
                .unwrap();
        }
    }

    impl Drop for LogFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn active_older_sessions_are_found_and_latest_limit_event_wins() {
        let logs = LogFixture::new();
        for day in 2..=10 {
            logs.write(&format!("{day:02}"), 800, 10, day);
        }
        logs.write("01", 950, 40, 20);
        // A later unrelated write in another log must not win over a newer limit event.
        logs.write("11", 900, 20, 30);
        assert_eq!(
            newest_rollouts(&logs.0, 2)[1].file_name().unwrap(),
            "rollout-01.jsonl"
        );
        let (windows, written) = super::codex_log_usage(&logs.0).unwrap();
        assert_eq!(written, 950);
        assert_eq!(windows[0].remaining_percent, 60);
    }

    #[test]
    fn manual_refresh_bypasses_even_fresh_codex_logs() {
        let logs = LogFixture::new();
        logs.write("01", 950, 40, 1);
        let cached = super::codex_usage_with_refresh(&logs.0, false, 1000, || {
            panic!("fresh cache should be reused")
        })
        .unwrap();
        assert_eq!(cached.windows[0].remaining_percent, 60);
        assert_eq!(cached.collected_at, Some(950));
        let live =
            super::codex_usage_with_refresh(&logs.0, true, 1000, || Err("live lookup".to_owned()));
        assert_eq!(live.err().as_deref(), Some("live lookup"));
    }

    #[test]
    fn stale_or_partly_reset_codex_logs_require_live_limits() {
        let logs = LogFixture::new();
        logs.write("01", 950, 40, 1);
        for now in [1250, 3000] {
            let live = super::codex_usage_with_refresh(&logs.0, false, now, || {
                Err("live lookup".to_owned())
            });
            assert_eq!(live.err().as_deref(), Some("live lookup"));
        }
        logs.write("01", 2950, 40, 1);
        let live =
            super::codex_usage_with_refresh(&logs.0, false, 3000, || Err("live lookup".to_owned()));
        assert_eq!(live.err().as_deref(), Some("live lookup"));
    }

    #[test]
    #[ignore = "requires a locally installed and logged-in Codex CLI"]
    fn fetches_live_codex_usage() {
        let usage = super::fetch_codex_usage(true).expect("live Codex lookup should succeed");
        println!("status={} windows={}", usage.status, usage.windows.len());
        assert_eq!(usage.status, "available");
        println!("CODEX_WIRE {}", serde_json::to_string(&usage).unwrap());
        for window in usage.windows {
            println!(
                "{} {}% resets_at={:?}",
                window.label, window.remaining_percent, window.resets_at
            );
        }
    }

    #[test]
    #[ignore = "requires local Codex session logs; run manually with `cargo test -- --ignored reads_live_codex_log`"]
    fn reads_live_codex_log() {
        let (windows, written) = super::codex_log_usage(&super::codex_sessions_dir())
            .expect("a recent Codex log with unexpired limits");
        println!("written={written} windows={}", windows.len());
        for window in &windows {
            println!(
                "{} {}% resets_at={:?}",
                window.label, window.remaining_percent, window.resets_at
            );
        }
        assert!(!windows.is_empty());
    }

    #[test]
    #[ignore = "requires local Codex session logs; verifies the remote script on this machine's sh"]
    fn remote_script_prints_a_parsable_line() {
        let script = format!(
            "{}; {}",
            super::remote_account_probe(),
            super::REMOTE_CODEX_LOG_SCRIPT
        );
        let output = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(super::remote_account(&text).is_some());
        assert!(codex_usage_from_log(&text).is_some());
    }

    #[test]
    fn remote_host_ids_are_stable_and_url_safe() {
        let id = remote_host_id("managed:abc");
        assert_eq!(id, remote_host_id("managed:abc"));
        assert_ne!(id, remote_host_id("managed:abd"));
        assert!(
            id.starts_with("ssh-") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
    }

    #[test]
    fn relative_reset_becomes_seconds() {
        assert_eq!(
            parse_relative_duration("Resets in 3h 12m"),
            Some(3 * 3_600 + 12 * 60)
        );
        assert_eq!(parse_relative_duration("Resets in 45m"), Some(45 * 60));
        assert_eq!(
            parse_relative_duration("Resets in 2d 3h"),
            Some(2 * 86_400 + 3 * 3_600)
        );
        assert_eq!(parse_relative_duration("Resets Sep 21 at 7pm"), None);
    }

    #[test]
    fn absolute_resets_use_the_printed_zone_and_never_roll_expired_times_forward() {
        let collected_at = epoch("2026-10-22T15:00:00Z");
        for (text, expected) in [
            ("Resets 7pm (Asia/Seoul)", "2026-10-23T10:00:00Z"),
            ("Resets 3:30am (Asia/Kuala_Lumpur)", "2026-10-21T19:30:00Z"),
            (
                "Resets Oct 26 at 7:59pm (Asia/Seoul)",
                "2026-10-26T10:59:00Z",
            ),
            ("Resets Oct 21 at 7pm (Asia/Seoul)", "2026-10-21T10:00:00Z"),
            ("Resets Dec 31 2025 at 19:00 (UTC)", "2025-12-31T19:00:00Z"),
        ] {
            assert_eq!(
                parse_reset_instant(text, collected_at),
                Some(epoch(expected)),
                "{text}"
            );
        }
        assert_eq!(
            parse_reset_instant("Resets 7pm (Unknown/Zone)", collected_at),
            None
        );
        assert_eq!(
            parse_reset_instant("Resets Nov 1 at 1:30am (America/New_York)", collected_at),
            None
        );
        assert_eq!(
            parse_reset_instant("Resets Mar 8 at 2:30am (America/New_York)", collected_at),
            None
        );
    }

    fn epoch(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn relative_resets_are_anchored_to_collection_not_upload() {
        assert_eq!(parse_reset_instant("Resets in 2h 13m", 1000), Some(8980));
        assert_eq!(
            parse_reset_instant("in 2 hours and 13 minutes", 1000),
            Some(8980)
        );
        assert_eq!(
            parse_relative_duration("Resets in 2h something unknown"),
            None
        );
        assert_eq!(
            parse_relative_duration("Resets in 9223372036854775807d"),
            None
        );
        assert_eq!(parse_relative_duration("Resets in 2 months"), None);
    }

    #[test]
    fn both_agents_serialize_the_same_seconds_and_fallback_contract() {
        let collected_at = epoch("2026-10-22T15:00:00Z");
        let claude = super::parse_claude_usage_at(
            "Current session 23% used Resets in 2h 13m\nCurrent week (all models) 41% used Resets Oct 26 at 7:59pm (Asia/Seoul)\nCurrent week (Sonnet only) 80% remaining Resets after a provider event with more than eight original words",
            collected_at,
        );
        assert_eq!(claude.collected_at, Some(collected_at));
        let json = serde_json::to_value(&claude).unwrap();
        assert_eq!(json["windows"][0]["resetsAt"], collected_at + 7980);
        assert_eq!(
            json["windows"][1]["resetsAt"],
            epoch("2026-10-26T10:59:00Z")
        );
        assert!(json["windows"][0].get("resetText").is_none());
        assert!(json["windows"][2].get("resetsAt").is_none());
        assert_eq!(
            json["windows"][2]["resetText"],
            "Resets after a provider event with more than eight original words"
        );
        let mut codex = Vec::new();
        for value in [
            serde_json::json!({"usedPercent": 23.0, "resetsAt": (collected_at + 7980) * 1000}),
            serde_json::json!({"usedPercent": 41, "resetText": "Resets Oct 26 at 7:59pm (Asia/Seoul)"}),
            serde_json::json!({"usedPercent": 20, "resetsAt": null, "resetText": "Resets after a provider event"}),
            serde_json::json!({"usedPercent": 0, "resetsAt": null, "resetText": null}),
        ] {
            super::append_window(&mut codex, "test", &value, collected_at);
        }
        let json = serde_json::to_value(&codex).unwrap();
        assert_eq!(json[0]["resetsAt"], collected_at + 7980);
        assert_eq!(json[1]["resetsAt"], epoch("2026-10-26T10:59:00Z"));
        assert!(json[2].get("resetsAt").is_none());
        assert!(json[3].get("resetsAt").is_none());
        assert!(json[3].get("resetText").is_none());
    }

    #[test]
    fn logs_without_a_source_timestamp_are_not_restamped_as_fresh() {
        let value = serde_json::json!({"payload": {"type": "token_count", "rate_limits": {"primary": {"used_percent": 5, "resets_at": 3000}}}});
        assert!(codex_usage_from_log(&value.to_string()).is_none());
    }

    #[test]
    fn claude_session_window_gets_a_countdown_like_codex() {
        let usage =
            parse_claude_usage("Current session 23% used Resets 3:30am (Asia/Kuala_Lumpur)");
        let window = &usage.windows[0];
        assert!(window.resets_at.is_some());
        assert!(window.reset_text.is_none());
    }

    #[test]
    #[ignore = "requires a locally installed and logged-in Claude Code CLI; run manually with `cargo test -- --ignored fetches_live_claude_usage`"]
    fn fetches_live_claude_usage() {
        // A directory the developer running this test has almost certainly
        // already trusted interactively: this very repository (three levels
        // up from `apps/desktop/src-tauri`, the crate's manifest directory).
        let cwd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .map(|path| path.to_string_lossy().into_owned());
        let usage = fetch_claude_usage(cwd).expect("claude usage collection should succeed");
        println!("status={} message={:?}", usage.status, usage.message);
        assert_eq!(usage.status, "available", "message: {:?}", usage.message);
        assert!(!usage.windows.is_empty());
        println!("CLAUDE_WIRE {}", serde_json::to_string(&usage).unwrap());
        for window in &usage.windows {
            println!(
                "{} -> {}% remaining",
                window.label, window.remaining_percent
            );
        }
    }

    #[test]
    #[ignore = "collects real CLI limits and uploads them using the desktop's existing signed-in backend session"]
    fn uploads_live_usage_and_verifies_server_round_trip() {
        // Keep the same device/host row as the app, so acceptance checks do
        // not leave synthetic hosts in the user's usage list.
        let auth = serde_json::to_value(crate::auth::google_auth_status()).unwrap();
        let device = auth["deviceId"]
            .as_str()
            .expect("desktop must be signed in");
        let host_id = format!("device-{device}");
        let cwd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .map(|path| path.to_string_lossy().into_owned());
        let mut verified = Vec::new();
        for agent in ["codex", "claude"] {
            let account_id = super::local_account_id(agent).expect("CLI must be signed in");
            let usage = match agent {
                "codex" => super::fetch_codex_usage(true),
                _ => super::fetch_claude_usage(cwd.clone()),
            }
            .expect("live collection must succeed");
            assert_eq!(usage.status, "available", "{:?}", usage.message);
            assert_eq!(Some(&account_id), super::local_account_id(agent).as_ref());
            for label in ["5h", "Week"] {
                assert!(
                    usage
                        .windows
                        .iter()
                        .any(|window| window.label == label && window.resets_at.is_some()),
                    "{agent} {label} missing reset"
                );
            }
            let snapshot = super::UsageSnapshot {
                agent_id: agent.to_owned(),
                host_id: host_id.clone(),
                host_label: "This device".to_owned(),
                account_id: Some(account_id),
                account_email: super::local_account(agent).and_then(|a| a.label),
                collected_at: usage.collected_at.unwrap(),
                windows: usage.windows,
            };
            let payload = serde_json::to_value(&snapshot).unwrap();
            tauri::async_runtime::block_on(crate::auth::put_usage_snapshot(payload.clone()))
                .expect("upload must succeed");
            let response = tauri::async_runtime::block_on(crate::auth::list_usage_snapshots())
                .expect("GET must succeed");
            let saved = response["snapshots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["agentId"] == agent && row["accountId"] == payload["accountId"])
                .expect("uploaded account must be returned");
            assert_eq!(saved["accountEmail"], payload["accountEmail"]);
            assert_eq!(saved["windows"], payload["windows"]);
            assert_eq!(
                saved["collectedAt"].as_f64(),
                payload["collectedAt"].as_i64().map(|at| at as f64)
            );
            println!("VERIFIED_SNAPSHOT {payload}");
            verified.push(payload);
        }
        if let Ok(path) = std::env::var("FASTADE_USAGE_VERIFICATION_JSON") {
            std::fs::write(path, serde_json::to_vec_pretty(&verified).unwrap()).unwrap();
        }
    }

    #[test]
    fn parses_claude_subscription_windows() {
        let usage = parse_claude_usage(
            "Current session 23% used Resets in 3h 12m\nCurrent week (all models) 41% used Resets Sep 21\nCurrent week (Sonnet only) 80% remaining Resets Sep 22",
        );
        assert_eq!(usage.status, "available");
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].remaining_percent, 77);
        assert_eq!(usage.windows[1].remaining_percent, 59);
        assert_eq!(usage.windows[2].remaining_percent, 80);
    }

    #[test]
    fn recognizes_api_billing_without_subscription_limit() {
        let usage = parse_claude_usage("Claude Code · API Usage Billing Total cost: $0.0000");
        assert_eq!(usage.status, "unavailable");
        assert!(usage.message.unwrap().contains("API billing"));
    }

    #[test]
    fn removes_terminal_control_sequences() {
        assert_eq!(terminal_text("A\x1b[10GB\x1b]0;title\x07C"), "A BC");
    }
}
