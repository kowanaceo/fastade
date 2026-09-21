use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::servers::{resolve_managed_server, AuthMethod};
use crate::session::shell_quote;

enum SshTarget {
    /// A `~/.ssh/config` Host alias, used as-is.
    Alias(String),
    Managed {
        host: String,
        port: u16,
        key_path: Option<String>,
        username: String,
        password: Option<String>,
    },
}

/// Turns a session `endpoint` (`"managed:<id>"` or a `~/.ssh/config` alias)
/// into connection details, resolving a managed server's stored credential
/// the same way the interactive session launcher does.
fn resolve_target(endpoint: &str, app: &tauri::AppHandle) -> Result<SshTarget, String> {
    Ok(if let Some(id) = endpoint.strip_prefix("managed:") {
        let (server, password) = resolve_managed_server(app, id)?;
        SshTarget::Managed {
            host: server.host,
            port: server.port,
            key_path: match server.auth {
                AuthMethod::KeyFile { path } => Some(path),
                AuthMethod::Password => None,
            },
            username: server.username,
            password,
        }
    } else {
        SshTarget::Alias(endpoint.to_owned())
    })
}

/// Applies a resolved target's connection args to an `ssh` command and
/// returns its password, if any — the caller still needs to wire up an
/// `AskpassHelper` for it, since building one takes I/O that can fail.
fn apply_target(command: &mut Command, target: &SshTarget) -> Option<String> {
    match target {
        SshTarget::Alias(alias) => {
            // No stored credential to fall back on for a plain ~/.ssh/config
            // host — keep refusing to hang on a prompt.
            command.arg("-o").arg("BatchMode=yes");
            command.arg(alias);
            None
        }
        SshTarget::Managed {
            host,
            port,
            key_path,
            username,
            password,
        } => {
            command.args(["-p", &port.to_string()]);
            if let Some(path) = key_path {
                command.arg("-o").arg("BatchMode=yes");
                command.args(["-i", path]);
            } else if password.is_none() {
                command.arg("-o").arg("BatchMode=yes");
            }
            command.arg(format!("{username}@{host}"));
            password.clone()
        }
    }
}

/// Streams a local file into `remote_path` on a remote SSH device, creating
/// its parent directory first. Uses the same non-interactive `ssh` +
/// `SSH_ASKPASS` approach as [`list_remote_directory`] — a one-off exec, not
/// the interactive session PTY — piping the file over stdin instead of
/// depending on a separate `scp` binary.
pub fn upload_file(
    app: &tauri::AppHandle,
    endpoint: &str,
    local_path: &Path,
    remote_path: &str,
) -> Result<(), String> {
    let target = resolve_target(endpoint, app)?;
    let remote_dir = Path::new(remote_path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty());
    let script = match remote_dir {
        Some(dir) => format!(
            "mkdir -p -- {} && cat > {}",
            shell_target(&dir.to_string_lossy()),
            shell_target(remote_path)
        ),
        None => format!("cat > {}", shell_target(remote_path)),
    };
    let mut command = Command::new("ssh");
    command.arg("-o").arg("ConnectTimeout=8");
    let password = apply_target(&mut command, &target);
    let askpass = password.as_deref().map(AskpassHelper::write).transpose()?;
    if let Some(helper) = &askpass {
        command.env("SSH_ASKPASS", helper.script_path());
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env_remove("DISPLAY");
    }
    let mut child = command
        .args(["--", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("SSH connection failed: {error}"))?;
    {
        let mut source = fs::File::open(local_path).map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "failed to open ssh stdin".to_owned())?;
        std::io::copy(&mut source, stdin).map_err(|error| error.to_string())?;
    }
    child.stdin = None; // close stdin so the remote `cat` sees EOF
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    drop(askpass);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(if message.is_empty() {
            format!("Could not write the remote file: {remote_path}")
        } else {
            message.to_owned()
        });
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDirectoryListing {
    /// The resolved absolute path actually listed (`~` and relative
    /// navigation resolved by the remote shell itself).
    path: String,
    /// Immediate subdirectory names, sorted.
    entries: Vec<String>,
}

/// Lists the immediate subdirectories of `path` on a remote SSH device, so
/// the UI can offer a click-to-browse folder picker for remote projects
/// instead of requiring the path to be typed blind (VS Code Remote-SSH
/// style). Uses a one-off non-interactive `ssh` exec — separate from any
/// live interactive session PTY — so browsing never disturbs a running shell.
#[tauri::command]
pub async fn list_remote_directory(
    endpoint: String,
    path: String,
    app: tauri::AppHandle,
) -> Result<RemoteDirectoryListing, String> {
    let requested_path = if path.trim().is_empty() {
        "~".to_owned()
    } else {
        path
    };
    // A managed server's `endpoint` is `managed:<id>`, not something `ssh`
    // can resolve as a hostname on its own — turn it into real connection
    // args first, same as the interactive session launcher does.
    let target = resolve_target(&endpoint, &app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let script = format!(
            "cd -- {} 2>/dev/null && pwd && find -L . -mindepth 1 -maxdepth 1 -type d -printf '%f\\n' 2>/dev/null | LC_ALL=C sort",
            shell_target(&requested_path)
        );
        let mut command = Command::new("ssh");
        command.arg("-o").arg("ConnectTimeout=8");
        let password = apply_target(&mut command, &target);
        // This is a one-off, non-interactive exec with no PTY to type a
        // password into (unlike an open session, which watches for the
        // prompt in its terminal). SSH_ASKPASS_REQUIRE=force makes ssh use a
        // helper program for the password instead of a tty, so browsing
        // still works for a password-auth managed server.
        let askpass = password.as_deref().map(AskpassHelper::write).transpose()?;
        if let Some(helper) = &askpass {
            command.env("SSH_ASKPASS", helper.script_path());
            command.env("SSH_ASKPASS_REQUIRE", "force");
            command.env_remove("DISPLAY");
        }
        let output = command
            .args(["--", &script])
            .output()
            .map_err(|error| format!("SSH connection failed: {error}"))?;
        drop(askpass);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr.trim();
            return Err(if message.is_empty() {
                format!("Could not open the folder: {requested_path}")
            } else {
                message.to_owned()
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let resolved_path = lines.next().unwrap_or(&requested_path).to_owned();
        let entries = lines.map(str::to_owned).collect();
        Ok(RemoteDirectoryListing {
            path: resolved_path,
            entries,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// A throwaway `SSH_ASKPASS` helper script that just prints one password.
/// Lives in its own directory with owner-only permissions, deleted as soon
/// as the `ssh` call finishes (success or not) via `Drop`.
struct AskpassHelper {
    directory: PathBuf,
}

impl AskpassHelper {
    fn write(password: &str) -> Result<Self, String> {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("fastade-askpass-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).map_err(|error| error.to_string())?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let password_path = directory.join("password");
        fs::write(&password_path, password).map_err(|error| error.to_string())?;
        fs::set_permissions(&password_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        let script_path = directory.join("askpass.sh");
        fs::write(
            &script_path,
            format!(
                "#!/bin/sh\ncat {}\n",
                shell_quote(&password_path.to_string_lossy())
            ),
        )
        .map_err(|error| error.to_string())?;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        Ok(Self { directory })
    }

    fn script_path(&self) -> PathBuf {
        self.directory.join("askpass.sh")
    }
}

impl Drop for AskpassHelper {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Shell-quotes a path for use as a `cd` target, but keeps a leading `~` (or
/// `~/`) unquoted so the remote shell still expands it to the home
/// directory — `shell_quote` alone would wrap it in single quotes, which
/// turns `~` into a literal, nonexistent directory name instead.
fn shell_target(path: &str) -> String {
    if path == "~" {
        return "~".to_owned();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("~/{}", shell_quote(rest));
    }
    shell_quote(path)
}

#[cfg(test)]
mod tests {
    use super::{shell_target, AskpassHelper};
    use std::process::Command;

    #[test]
    fn keeps_tilde_unquoted_for_expansion() {
        assert_eq!(shell_target("~"), "~");
        assert_eq!(shell_target("~/work"), "~/'work'");
        assert_eq!(shell_target("~/it's"), "~/'it'\"'\"'s'");
    }

    #[test]
    fn quotes_absolute_paths_fully() {
        assert_eq!(shell_target("/root/my project"), "'/root/my project'");
    }

    #[test]
    fn askpass_script_prints_the_password_and_cleans_up_after_itself() {
        let helper = AskpassHelper::write("it's a secret").expect("should write helper");
        let output = Command::new(helper.script_path())
            .output()
            .expect("should run the helper script");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "it's a secret");
        let directory = helper.directory.clone();
        drop(helper);
        assert!(!directory.exists(), "temp directory should be cleaned up");
    }
}
