//! Sets up an SSH host so an AI CLI running there can use fastade's MCP tools
//! and hooks. The desktop app's socket is forwarded into each session (see
//! `build_command`), so the host needs only this relay script — no binary for
//! its architecture and no credential: authentication and the local-or-account
//! storage choice stay in the desktop app.
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use crate::{mcp_settings, remote_fs, session::shell_quote};

const BRIDGE_SCRIPT: &str = include_str!("remote/fastade_mcp.py");
const REMOTE_BRIDGE: &str = ".fastade/bin/fastade-mcp";

/// Unguessable (it embeds the session's UUID) and created owner-only by sshd,
/// so another user on a shared host cannot squat or reach it.
pub(crate) fn remote_socket_path(session_id: &str) -> String {
    format!("/tmp/fastade-{session_id}.sock")
}

/// Once per host and set of enabled agents per app run, off the session's
/// start path: the session opens immediately and the hooks land a moment
/// later. A failed attempt is forgotten so the next session retries.
pub(crate) fn ensure_installed_in_background(app: &tauri::AppHandle, endpoint: &str) {
    static DONE: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let agents = mcp_settings::enabled_agents();
    if agents.is_empty() {
        return;
    }
    let key = format!(
        "{endpoint}|{}",
        agents
            .iter()
            .map(|cli| format!("{cli:?}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    {
        let Ok(mut done) = DONE.lock() else { return };
        if !done.get_or_insert_with(HashSet::new).insert(key.clone()) {
            return;
        }
    }
    let app = app.clone();
    let endpoint = endpoint.to_owned();
    thread::spawn(move || {
        if let Err(error) = ensure_installed(&app, &endpoint, &agents) {
            eprintln!("fastade: could not set up MCP on {endpoint}: {error}");
            if let Ok(mut done) = DONE.lock() {
                if let Some(done) = done.as_mut() {
                    done.remove(&key);
                }
            }
        }
    });
}

fn ensure_installed(
    app: &tauri::AppHandle,
    endpoint: &str,
    agents: &[crate::session::CliKind],
) -> Result<(), String> {
    let home = remote_fs::run_remote_script(
        endpoint,
        "if command -v python3 >/dev/null 2>&1; then printf %s \"$HOME\"; fi",
        app,
    )?;
    if home.trim().is_empty() || !home.starts_with('/') {
        return Err("python3 is required on the remote host".to_owned());
    }
    let bridge = format!("{home}/{REMOTE_BRIDGE}");

    let staging = std::env::temp_dir().join(format!("fastade-remote-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let result = install(app, endpoint, agents, &home, &bridge, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn install(
    app: &tauri::AppHandle,
    endpoint: &str,
    agents: &[crate::session::CliKind],
    home: &str,
    bridge: &str,
    staging: &Path,
) -> Result<(), String> {
    let script_path = staging.join("fastade-mcp");
    fs::write(&script_path, BRIDGE_SCRIPT).map_err(|error| error.to_string())?;
    remote_fs::upload_file(app, endpoint, &script_path, bridge)?;
    remote_fs::run_remote_script(
        endpoint,
        &format!("chmod 755 -- {}", shell_quote(bridge)),
        app,
    )?;

    // Pull each config the integration edits into a home-shaped directory.
    let root = staging.join("home");
    let mut originals: HashMap<&str, String> = HashMap::new();
    for cli in agents {
        for relative in mcp_settings::config_files(*cli) {
            if originals.contains_key(relative) {
                continue;
            }
            let remote = format!("{home}/{relative}");
            let contents = remote_fs::run_remote_script(
                endpoint,
                &format!("cat -- {} 2>/dev/null || true", shell_quote(&remote)),
                app,
            )?;
            if !contents.is_empty() {
                let local = root.join(relative);
                fs::create_dir_all(local.parent().unwrap_or(&root))
                    .map_err(|error| error.to_string())?;
                fs::write(&local, &contents).map_err(|error| error.to_string())?;
            }
            originals.insert(relative, contents);
        }
    }

    for cli in agents {
        mcp_settings::configure_under(&root, *cli, Path::new(bridge))?;
    }

    // Send back only what changed, so an already-set-up host is left alone.
    for (relative, original) in &originals {
        let local: PathBuf = root.join(relative);
        let Ok(updated) = fs::read_to_string(&local) else {
            continue;
        };
        if &updated != original {
            remote_fs::upload_file(app, endpoint, &local, &format!("{home}/{relative}"))?;
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::net::UnixListener,
        process::{Command, Stdio},
    };

    #[test]
    fn socket_path_is_per_session() {
        assert_eq!(remote_socket_path("abc"), "/tmp/fastade-abc.sock");
    }

    #[test]
    fn registers_the_bridge_in_remote_style_config_files() {
        let root = std::env::temp_dir().join(format!("fastade-remote-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".claude")).unwrap();
        fs::write(root.join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
        let bridge = Path::new("/home/u/.fastade/bin/fastade-mcp");
        for cli in [
            crate::session::CliKind::Claude,
            crate::session::CliKind::Codex,
        ] {
            mcp_settings::configure_under(&root, cli, bridge).unwrap();
        }
        let claude: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(claude["theme"], "dark");
        assert_eq!(claude["mcpServers"]["fastade"]["command"], bridge.to_str().unwrap());
        assert!(fs::read_to_string(root.join(".claude/settings.json"))
            .unwrap()
            .contains("report-activity"));
        assert!(fs::read_to_string(root.join(".codex/config.toml"))
            .unwrap()
            .contains("fastade"));
        let _ = fs::remove_dir_all(root);
    }

    /// Runs the real script against a fake app socket: tools/list is relayed
    /// from the app, tools/call carries the caller's session id, and a hook
    /// reports the activity state.
    #[test]
    fn python_bridge_relays_tools_and_hooks_to_the_app_socket() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        // A unix socket path must fit in SUN_LEN (104 bytes on macOS), which the
        // default temp directory there does not.
        let dir = PathBuf::from("/tmp").join(format!("fpy-{}", &uuid::Uuid::new_v4().to_string()[..8]));
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("app.sock");
        let script = dir.join("fastade-mcp");
        fs::write(&script, BRIDGE_SCRIPT).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let requests = std::sync::Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let seen = requests.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let reply = match request["op"].as_str() {
                    Some("tools") => serde_json::json!({"ok":true,"tools":[{"name":"create_record"}]}),
                    _ => serde_json::json!({"ok":true}),
                };
                seen.lock().unwrap().push(request);
                let mut stream = stream;
                writeln!(stream, "{reply}").unwrap();
            }
        });

        let mut child = Command::new("python3")
            .arg(&script)
            .env("FASTADE_SOCK", &socket)
            .env("FASTADE_SESSION_ID", "s1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"create_record","arguments":{{"kind":"note","title":"t","session_id":"evil"}}}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"report_activity","arguments":{{}}}}}}"#
        )
        .unwrap();
        drop(stdin);
        let mut output = String::new();
        child.stdout.take().unwrap().read_to_string(&mut output).unwrap();
        child.wait().unwrap();
        let replies: Vec<serde_json::Value> =
            output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(replies[0]["result"]["tools"][0]["name"], "create_record");
        assert_eq!(replies[1]["result"]["isError"], false);
        assert_eq!(replies[2]["error"]["code"], -32602, "unlisted tools are refused");

        let hook = Command::new("python3")
            .args([script.to_str().unwrap(), "report-activity", "working"])
            .env("FASTADE_SOCK", &socket)
            .env("FASTADE_SESSION_ID", "s1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&hook.stdout).trim(), "{}");

        let seen = requests.lock().unwrap();
        let create = seen.iter().find(|r| r["op"] == "create_record").unwrap();
        assert_eq!(create["session_id"], "s1", "a caller cannot choose its session");
        assert_eq!(create["title"], "t");
        assert!(seen.iter().all(|r| r["op"] != "report_activity" || r["state"] == "working"));
        assert!(seen.iter().any(|r| r["op"] == "report_activity" && r["state"] == "working"));
        let _ = fs::remove_dir_all(dir);
    }
}
