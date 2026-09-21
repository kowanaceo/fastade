use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use crate::session::{most_recent_local_path, resolve_program, AppState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    label: String,
    remaining_percent: i64,
    resets_at: Option<i64>,
    reset_text: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    agent_id: String,
    status: &'static str,
    windows: Vec<UsageWindow>,
    message: Option<String>,
}

#[tauri::command]
pub async fn get_agent_usage(
    agent_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<AgentUsage, String> {
    let requested_id = agent_id.clone();
    // Claude Code's interactive TUI shows a one-time "do you trust this
    // folder?" prompt the first time it runs in a directory, and it can't be
    // answered by this headless probe. Launch it from wherever the user most
    // recently ran a local session (almost certainly already trusted, since
    // they were using Claude Code there) instead of an unpredictable
    // inherited working directory, which made this fail far more often than
    // the equivalent Codex lookup.
    let claude_cwd = most_recent_local_path(&state);
    tauri::async_runtime::spawn_blocking(move || match requested_id.as_str() {
        "codex" => fetch_codex_usage(),
        "claude" => fetch_claude_usage(claude_cwd),
        _ => Ok(AgentUsage {
            agent_id: requested_id,
            status: "unavailable",
            windows: Vec::new(),
            message: Some("Check the Usage page.".to_owned()),
        }),
    })
    .await
    .map_err(|error| error.to_string())?
}

fn fetch_codex_usage() -> Result<AgentUsage, String> {
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
    let _ = child.kill();
    let _ = child.wait();
    let result = received.map_err(|_| "Codex usage lookup timed out.".to_owned())?;
    let result = result
        .and_then(|message| message.get("result").cloned())
        .ok_or_else(|| "Did not receive a Codex usage response.".to_owned())?;
    let limits = &result["rateLimits"];
    let mut windows = Vec::new();
    append_window(&mut windows, "5h", &limits["primary"]);
    append_window(&mut windows, "Week", &limits["secondary"]);
    Ok(AgentUsage {
        agent_id: "codex".to_owned(),
        status: if windows.is_empty() {
            "unavailable"
        } else {
            "available"
        },
        windows,
        message: None,
    })
}

fn append_window(windows: &mut Vec<UsageWindow>, label: &str, value: &Value) {
    let Some(used_percent) = value.get("usedPercent").and_then(Value::as_i64) else {
        return;
    };
    windows.push(UsageWindow {
        label: label.to_owned(),
        remaining_percent: (100 - used_percent).clamp(0, 100),
        resets_at: value.get("resetsAt").and_then(Value::as_i64),
        reset_text: None,
    });
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
    Ok(parse_claude_usage(&text))
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
    claude_trust_map()?.into_iter().find_map(|(dir, trusted)| {
        (trusted && std::path::Path::new(&dir).is_dir()).then_some(dir)
    })
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

fn parse_claude_usage(text: &str) -> AgentUsage {
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
                // Codex reports a reset instant, so its remaining-time is
                // computed on the frontend from resets_at. Claude only
                // prints text ("Resets 3:30am (Asia/...)", "Resets Sep 21
                // at 7pm (Asia/...)", or occasionally "Resets in 3h 12m");
                // parse it into the same resets_at so both agents render
                // identically, and only fall back to raw text if it doesn't
                // match a pattern this understands.
                let resets_at = reset_text.as_deref().and_then(parse_reset_instant);
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
/// relative "in ..." duration from now, or an absolute clock time / date +
/// time. Claude Code runs on this machine, so an absolute time is assumed
/// to already be in the system's local timezone — the "(Region/City)" it
/// prints alongside is just naming that zone, not a different one to
/// convert from.
fn parse_reset_instant(text: &str) -> Option<i64> {
    if let Some(seconds) = parse_relative_duration(text) {
        return Some(now_unix() + seconds);
    }
    parse_absolute_reset(text)
}

fn parse_absolute_reset(text: &str) -> Option<i64> {
    use chrono::{Datelike, Local, NaiveTime, TimeZone};

    let without_tz = text.split('(').next().unwrap_or(text).trim();
    let lower = without_tz.to_ascii_lowercase();
    let after_resets = lower.find("resets")? + "resets".len();
    let rest = without_tz[after_resets..].trim();
    if rest.is_empty() {
        return None;
    }

    let (date_part, time_part) = match rest.split_once(" at ") {
        Some((date, time)) => (Some(date.trim()), Some(time.trim())),
        None if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) => (None, Some(rest)),
        None => (Some(rest), None),
    };

    let now = Local::now();
    let date = match date_part {
        Some(date_text) => parse_month_day(date_text, now.year())?,
        None => now.date_naive(),
    };
    let time = match time_part {
        Some(time_text) => parse_clock_time(time_text)?,
        None => NaiveTime::from_hms_opt(0, 0, 0)?,
    };

    let mut instant = Local.from_local_datetime(&date.and_time(time)).single()?;
    // A bare time with no date ("Resets 3:30am") means the next occurrence
    // of it, which is tomorrow if that time has already passed today.
    if date_part.is_none() && instant <= now {
        instant += chrono::Duration::days(1);
    }
    Some(instant.timestamp())
}

fn parse_clock_time(text: &str) -> Option<chrono::NaiveTime> {
    let mut normalized = text.trim().to_ascii_uppercase().replace(' ', "");
    if !normalized.contains(':') {
        let split_at = normalized.len().checked_sub(2)?;
        normalized.insert_str(split_at, ":00");
    }
    chrono::NaiveTime::parse_from_str(&normalized, "%I:%M%p").ok()
}

fn parse_month_day(text: &str, year: i32) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(&format!("{} {year}", text.trim()), "%b %d %Y").ok()
}

/// Parses a relative reset phrase like "in 3h 12m", "in 45m", "in 2d 3h"
/// into a second count, by scanning "<number><unit>" tokens (d/h/m/s) after
/// the word "in". Returns `None` for anything else (an absolute date/time).
fn parse_relative_duration(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let after_in = lower.find("in ")? + 3;
    let bytes = lower.as_bytes();
    let mut index = after_in;
    let mut seconds = 0_i64;
    let mut matched = false;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let Ok(number) = lower[start..index].parse::<i64>() else {
            break;
        };
        let unit_seconds = match bytes.get(index) {
            Some(b'd') => 86_400,
            Some(b'h') => 3_600,
            Some(b'm') => 60,
            Some(b's') => 1,
            _ => break,
        };
        seconds += number * unit_seconds;
        matched = true;
        index += 1;
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
        .take(8)
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

#[cfg(test)]
mod tests {
    use super::{
        fetch_claude_usage, now_unix, parse_claude_usage, parse_relative_duration,
        parse_reset_instant, terminal_text,
    };

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
    fn absolute_clock_time_resolves_to_the_next_occurrence() {
        let now = now_unix();
        let resets_at = parse_reset_instant("Resets 3:30am (Asia/Kuala_Lumpur)").unwrap();
        assert!(resets_at > now);
        assert!(
            resets_at <= now + 86_400,
            "should be within a day, got {}s away",
            resets_at - now
        );
    }

    #[test]
    fn absolute_date_and_time_resolves_to_a_future_instant() {
        let now = now_unix();
        let resets_at = parse_reset_instant("Resets Sep 21 at 7pm (Asia/Kuala_Lumpur)").unwrap();
        assert!(resets_at > now - 400 * 86_400);
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
        for window in &usage.windows {
            println!(
                "{} -> {}% remaining",
                window.label, window.remaining_percent
            );
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
