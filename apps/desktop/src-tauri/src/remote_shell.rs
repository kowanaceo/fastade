//! Remote shells that end with their SSH connection, and agent launch preflight.
use crate::{remote_fs, session::shell_quote};
use serde_json::Value;

const HELPER: &str = include_str!("remote/remote_shell.py");

pub fn helper_command(options: Value) -> String {
    format!("python3 -c {} {}", shell_quote(HELPER), shell_quote(&options.to_string()))
}

pub fn run(app: &tauri::AppHandle, endpoint: &str, options: Value) -> Result<Value, String> {
    if endpoint == "local" { return Err("Choose an SSH host".into()); }
    let output = remote_fs::run_remote_script(endpoint, &helper_command(options), app)?;
    serde_json::from_str(&output).map_err(|e| format!("Invalid remote shell response: {e}"))
}
