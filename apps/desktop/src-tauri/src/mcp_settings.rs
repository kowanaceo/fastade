//! Registers (or unregisters) the `fastade_mcp` stdio bridge in each AI
//! CLI's own MCP config, so that CLI can read fastade's session snapshot via
//! `list_sessions` — and, where the CLI
//! supports it, also wires up its hooks/notify mechanism to report accurate
//! working/waiting/idle status back to fastade instead of relying only on the
//! PTY-text heuristic. One toggle per agent in Settings, mirroring how each
//! CLI already lets a user add MCP servers or hooks by hand — we just
//! automate editing the same files.
use crate::session::{shell_quote, CliKind};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The already-built `fastade_mcp` binary, expected to sit next to this
/// app's own executable — true in `cargo tauri dev` (both land in the same
/// `target/debug`) and true for a bundled app once `fastade_mcp` is wired
/// up as a Tauri sidecar/externalBin for release builds.
pub fn mcp_binary_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "could not resolve the app's own directory".to_owned())?;
    let candidate = dir.join("fastade_mcp");
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(format!(
        "fastade_mcp binary not found at {} — build it with `cargo build --bin fastade_mcp`",
        candidate.display()
    ))
}

#[tauri::command]
pub fn mcp_agent_status(cli: CliKind) -> Result<bool, String> {
    let enabled = match cli {
        CliKind::Claude => json_has_fastade(&claude_json_path()?)?,
        CliKind::Gemini => json_has_fastade(&gemini_json_path()?)?,
        CliKind::Codex => codex_has_fastade()?,
    };
    if !enabled {
        return Ok(false);
    }

    // Older fastade versions only registered the MCP server. Reconcile all
    // managed pieces when an already-enabled integration is discovered so an
    // upgrade cannot remain permanently stuck without lifecycle hooks.
    let binary = mcp_binary_path()?;
    match cli {
        CliKind::Claude => {
            set_json_entry(&claude_json_path()?, true, &binary)?;
            set_claude_hooks(true, &binary)?;
        }
        CliKind::Gemini => set_json_entry(&gemini_json_path()?, true, &binary)?,
        CliKind::Codex => {
            set_codex_toml(true, &binary)?;
            set_codex_hooks(true, &binary)?;
            set_codex_notify(true, &binary)?;
        }
    }
    Ok(true)
}

#[tauri::command]
pub fn set_mcp_agent_enabled(cli: CliKind, enabled: bool) -> Result<(), String> {
    let binary = mcp_binary_path()?;
    match cli {
        CliKind::Claude => {
            set_json_entry(&claude_json_path()?, enabled, &binary)?;
            set_claude_hooks(enabled, &binary)
        }
        // Gemini CLI's hook/event config format isn't confidently known
        // here, so only the MCP tool registration is automated for it —
        // it stays on the PTY-text heuristic for activity status.
        CliKind::Gemini => set_json_entry(&gemini_json_path()?, enabled, &binary),
        CliKind::Codex => {
            set_codex_toml(enabled, &binary)?;
            set_codex_hooks(enabled, &binary)?;
            // Keep Codex's simpler turn-complete notifier as an idle-state
            // fallback until the user reviews the richer lifecycle hooks.
            // A notifier configured by the user is never overwritten.
            set_codex_notify(enabled, &binary)
        }
    }
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "could not determine the home directory".to_owned())
}

fn claude_json_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".claude.json"))
}

fn gemini_json_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".gemini").join("settings.json"))
}

fn codex_toml_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".codex").join("config.toml"))
}

fn codex_hooks_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".codex").join("hooks.json"))
}

/// Claude Code's *hooks* live in `settings.json`, a different file from the
/// `mcpServers` registration in `~/.claude.json`.
fn claude_settings_path() -> Result<PathBuf, String> {
    Ok(home()?.join(".claude").join("settings.json"))
}

fn hook_command(binary: &Path, activity: &str) -> String {
    format!(
        "{} report-activity {activity}",
        shell_quote(&binary.to_string_lossy())
    )
}

/// Claude exposes a direct PermissionRequest lifecycle event. Notification
/// needs a matcher because it also fires for informational events such as
/// auth success and ordinary idle prompts, which must not turn the badge
/// purple.
const CLAUDE_HOOK_EVENTS: [(&str, &str, Option<&str>); 7] = [
    ("SessionStart", "idle", None),
    ("UserPromptSubmit", "working", None),
    ("PreToolUse", "working", None),
    ("PermissionRequest", "waiting", None),
    (
        "Notification",
        "waiting",
        Some("permission_prompt|agent_needs_input|elicitation_dialog|elicitation_url_dialog"),
    ),
    ("Stop", "idle", None),
    ("StopFailure", "idle", None),
];

/// Idempotent: re-running with `enabled: true` never duplicates an entry,
/// and `enabled: false` only ever removes the exact hook command this
/// function itself would have added, leaving any hooks the user configured
/// by hand untouched.
fn set_claude_hooks(enabled: bool, binary: &Path) -> Result<(), String> {
    set_claude_hooks_at(&claude_settings_path()?, enabled, binary)
}

fn set_claude_hooks_at(path: &Path, enabled: bool, binary: &Path) -> Result<(), String> {
    set_json_hooks_at(path, &CLAUDE_HOOK_EVENTS, enabled, binary, false)
}

fn set_json_hooks_at(
    path: &Path,
    events: &[(&str, &str, Option<&str>)],
    enabled: bool,
    binary: &Path,
    run_async: bool,
) -> Result<(), String> {
    let mut value = read_json(path)?;
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let object = value.as_object_mut().expect("just normalized to an object");
    let hooks_value = object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks_value.is_object() {
        *hooks_value = serde_json::json!({});
    }
    let hooks_object = hooks_value
        .as_object_mut()
        .expect("just normalized to an object");

    for &(event, activity, matcher) in events {
        let command = hook_command(binary, activity);
        // Remove every activity state previously managed by fastade. This also
        // migrates hooks written by older builds, where Stop/SessionStart used
        // `waiting` instead of `idle`, without touching user-owned hooks.
        let managed_commands =
            ["working", "waiting", "idle"].map(|state| hook_command(binary, state));
        let groups = hooks_object
            .entry(event.to_owned())
            .or_insert_with(|| serde_json::json!([]));
        if !groups.is_array() {
            *groups = serde_json::json!([]);
        }
        let array = groups.as_array_mut().expect("just normalized to an array");

        for group in array.iter_mut() {
            if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                hooks.retain(|hook| {
                    let existing = hook.get("command").and_then(Value::as_str);
                    !managed_commands
                        .iter()
                        .any(|managed| existing == Some(managed.as_str()))
                });
            }
        }
        array.retain(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map(|hooks| !hooks.is_empty())
                .unwrap_or(true)
        });

        if enabled {
            let mut hook = serde_json::json!({ "type": "command", "command": command });
            if run_async {
                hook["async"] = serde_json::json!(true);
            }
            let mut group = serde_json::json!({ "hooks": [hook] });
            if let Some(matcher) = matcher {
                group["matcher"] = serde_json::json!(matcher);
            }
            array.push(group);
        }
    }

    write_json(path, &value)
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    if contents.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&contents)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())
}

fn json_has_fastade(path: &Path) -> Result<bool, String> {
    let value = read_json(path)?;
    Ok(value
        .get("mcpServers")
        .and_then(|servers| servers.get("fastade"))
        .is_some())
}

/// Only ever touches the `mcpServers.fastade` entry — every other key in
/// the file (the user's own settings, other MCP servers) is read back and
/// written out unchanged.
fn set_json_entry(path: &Path, enabled: bool, binary: &Path) -> Result<(), String> {
    let mut value = read_json(path)?;
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let object = value.as_object_mut().expect("just normalized to an object");
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    let servers_object = servers
        .as_object_mut()
        .expect("just normalized to an object");
    if enabled {
        servers_object.insert(
            "fastade".to_owned(),
            serde_json::json!({ "command": binary.to_string_lossy(), "args": [] }),
        );
    } else {
        servers_object.remove("fastade");
    }
    write_json(path, &value)
}

fn codex_has_fastade() -> Result<bool, String> {
    let path = codex_toml_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let doc = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("{} is not valid TOML: {error}", path.display()))?;
    Ok(doc
        .get("mcp_servers")
        .and_then(|servers| servers.get("fastade"))
        .is_some())
}

fn set_codex_toml(enabled: bool, binary: &Path) -> Result<(), String> {
    set_codex_toml_at(&codex_toml_path()?, enabled, binary)
}

fn set_codex_toml_at(path: &Path, enabled: bool, binary: &Path) -> Result<(), String> {
    let contents = if path.exists() {
        fs::read_to_string(path).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("{} is not valid TOML: {error}", path.display()))?;

    if enabled {
        let mut table = toml_edit::Table::new();
        table["command"] = toml_edit::value(binary.to_string_lossy().into_owned());
        table["args"] = toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new()));
        doc["mcp_servers"]["fastade"] = toml_edit::Item::Table(table);
    } else if let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(|item| item.as_table_like_mut())
    {
        servers.remove("fastade");
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, doc.to_string()).map_err(|error| error.to_string())
}

/// Codex lifecycle hooks provide both sides of the activity signal. A turn is
/// working from prompt submission through tool use. PermissionRequest is the
/// explicit user-blocked edge; Stop and Interrupt return to the composer.
/// SessionStart covers a freshly opened Codex UI before its first prompt.
const CODEX_HOOK_EVENTS: [(&str, &str, Option<&str>); 7] = [
    ("SessionStart", "idle", None),
    ("UserPromptSubmit", "working", None),
    ("PreToolUse", "working", None),
    ("PermissionRequest", "waiting", None),
    ("PostToolUse", "working", None),
    ("Stop", "idle", None),
    ("Interrupt", "idle", None),
];

fn set_codex_hooks(enabled: bool, binary: &Path) -> Result<(), String> {
    set_codex_hooks_at(&codex_hooks_path()?, enabled, binary)
}

fn set_codex_hooks_at(path: &Path, enabled: bool, binary: &Path) -> Result<(), String> {
    // Keep these synchronous: event order defines the badge state, and Codex
    // requires a valid JSON result from Stop/Interrupt command hooks.
    set_json_hooks_at(path, &CODEX_HOOK_EVENTS, enabled, binary, false)
}

fn set_codex_notify(enabled: bool, binary: &Path) -> Result<(), String> {
    set_codex_notify_at(&codex_toml_path()?, enabled, binary)
}

fn set_codex_notify_at(path: &Path, enabled: bool, binary: &Path) -> Result<(), String> {
    let contents = if path.exists() {
        fs::read_to_string(path).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("{} is not valid TOML: {error}", path.display()))?;

    let binary_string = binary.to_string_lossy().into_owned();
    let is_ours = doc
        .get("notify")
        .and_then(|item| item.as_array())
        .and_then(|array| array.iter().next())
        .and_then(|first| first.as_str())
        .map(|first| first == binary_string)
        .unwrap_or(false);

    if enabled && (doc.get("notify").is_none() || is_ours) {
        let mut array = toml_edit::Array::new();
        array.push(binary_string);
        array.push("report-activity");
        array.push("idle");
        doc["notify"] = toml_edit::Item::Value(toml_edit::Value::Array(array));
    } else if !enabled && is_ours {
        doc.as_table_mut().remove("notify");
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&path, doc.to_string()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn toggles_fastade_in_and_out_of_a_json_mcp_config_without_disturbing_other_keys() {
        let dir = std::env::temp_dir().join(format!("fastade-mcp-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#,
        )
        .unwrap();

        set_json_entry(&path, true, Path::new("/usr/local/bin/fastade_mcp")).unwrap();
        assert!(json_has_fastade(&path).unwrap());
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["mcpServers"]["other"]["command"], "x");
        assert_eq!(
            written["mcpServers"]["fastade"]["command"],
            "/usr/local/bin/fastade_mcp"
        );

        set_json_entry(&path, false, Path::new("/usr/local/bin/fastade_mcp")).unwrap();
        assert!(!json_has_fastade(&path).unwrap());
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["other"]["command"], "x");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn toggles_fastade_in_and_out_of_a_codex_toml_config_without_disturbing_other_keys() {
        let dir = std::env::temp_dir().join(format!("fastade-mcp-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "model = \"o1\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();

        set_codex_toml_at(&path, true, Path::new("/usr/local/bin/fastade_mcp")).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        let doc = contents.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(doc["model"].as_str().unwrap(), "o1");
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
            "x"
        );
        assert_eq!(
            doc["mcp_servers"]["fastade"]["command"].as_str().unwrap(),
            "/usr/local/bin/fastade_mcp"
        );

        set_codex_toml_at(&path, false, Path::new("/usr/local/bin/fastade_mcp")).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        let doc = contents.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(doc
            .get("mcp_servers")
            .and_then(|s| s.get("fastade"))
            .is_none());
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
            "x"
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    use std::fs;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fastade-mcp-hook-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn adds_and_removes_claude_hooks_idempotently_without_touching_other_hooks() {
        let dir = temp_dir();
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        let binary = Path::new("/usr/local/bin/fastade_mcp");

        set_claude_hooks_at(&path, true, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(
            written["hooks"]["Stop"][0]["hooks"][0]["command"],
            hook_command(binary, "idle")
        );
        assert_eq!(
            written["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            hook_command(binary, "working")
        );
        assert_eq!(
            written["hooks"]["Notification"][0]["matcher"],
            "permission_prompt|agent_needs_input|elicitation_dialog|elicitation_url_dialog"
        );
        assert_eq!(
            written["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            hook_command(binary, "waiting")
        );

        // Enabling again must not duplicate entries.
        set_claude_hooks_at(&path, true, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["Stop"].as_array().unwrap().len(), 1);

        set_claude_hooks_at(&path, false, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(written["hooks"]["Stop"].as_array().unwrap().len(), 0);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn adds_and_removes_codex_hooks_without_touching_other_hooks() {
        let dir = temp_dir();
        let path = dir.join("hooks.json");
        fs::write(
            &path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]},{"hooks":[{"type":"command","command":"'/usr/local/bin/fastade_mcp' report-activity waiting"}]}]}}"#,
        )
        .unwrap();
        let binary = Path::new("/usr/local/bin/fastade_mcp");

        set_codex_hooks_at(&path, true, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo mine"
        );
        assert_eq!(
            written["hooks"]["Stop"][1]["hooks"][0]["command"],
            hook_command(binary, "idle")
        );
        assert!(written["hooks"]["Stop"][1]["hooks"][0]
            .get("async")
            .is_none());
        assert_eq!(
            written["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            hook_command(binary, "working")
        );
        assert_eq!(
            written["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            hook_command(binary, "waiting")
        );
        assert_eq!(
            written["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            hook_command(binary, "working")
        );

        set_codex_hooks_at(&path, true, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["Stop"].as_array().unwrap().len(), 2);

        set_codex_hooks_at(&path, false, binary).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(
            written["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo mine"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn codex_notify_fallback_never_clobbers_a_user_notifier() {
        let dir = temp_dir();
        let path = dir.join("config.toml");
        let binary = Path::new("/usr/local/bin/fastade_mcp");

        fs::write(
            &path,
            "model = \"o1\"\nnotify = [\"/usr/local/bin/fastade_mcp\", \"report-activity\", \"waiting\"]\n",
        )
        .unwrap();
        set_codex_notify_at(&path, false, binary).unwrap();
        let doc = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert!(doc.get("notify").is_none());
        assert_eq!(doc["model"].as_str().unwrap(), "o1");

        // A user's own notify command must never be clobbered by disabling.
        fs::write(&path, "notify = [\"my-own-script\"]\n").unwrap();
        set_codex_notify_at(&path, true, binary).unwrap();
        let doc = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["notify"][0].as_str().unwrap(), "my-own-script");

        set_codex_notify_at(&path, false, binary).unwrap();
        let doc = fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["notify"][0].as_str().unwrap(), "my-own-script");

        fs::remove_dir_all(&dir).unwrap();
    }
}
