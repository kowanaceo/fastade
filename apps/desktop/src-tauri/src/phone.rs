//! Phone login for the aissh app: a dedicated ed25519 key whose public half is
//! added to `~/.ssh/authorized_keys`. The private key is handed to the UI once
//! (inside the QR) and is never kept on disk.
//!
//! With the desktop tunnel connected the phone cannot reach the forwarded port
//! directly (the backend binds it to loopback). It signs in to the backend's
//! jump account with the same key, which the backend limits to forwarding to
//! this device's tunnel port, and then authenticates to this Mac's sshd.

use crate::{auth, tunnel::SSH_KEYGEN};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Serialize;
use std::{
    fs,
    net::UdpSocket,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};
use uuid::Uuid;

const PHONE_PREFIX: &str = "aissh-phone";
/// Serializes read-modify-write cycles of `authorized_keys` within the app.
static AUTHORIZED_KEYS: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
pub struct PhoneJump {
    host: String,
    port: u16,
    user: String,
}

#[derive(Serialize)]
pub struct PhoneQr {
    name: String,
    /// The sshd to log in to, as seen from the jump host when there is one.
    host: String,
    port: u16,
    username: String,
    /// Set while the tunnel is connected; the phone connects through it.
    jump: Option<PhoneJump>,
    /// OpenSSH private key PEM, base64url encoded for the QR link.
    key: String,
}

struct NewKey {
    comment: String,
    private: String,
    public: String,
}

impl NewKey {
    /// `ssh-ed25519 <base64>` without the comment, as the backend requires.
    fn public_without_comment(&self) -> String {
        self.public.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Serialize, PartialEq, Debug)]
pub struct PhoneKey {
    /// The public key blob; identifies the entry for revocation.
    id: String,
    label: String,
}

fn ssh_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir().ok_or("Home directory not found.")?.join(".ssh"))
}

fn private_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| format!("Could not create {}: {error}", path.display()))
}

/// (blob, comment) of an `authorized_keys` line that belongs to a phone.
fn phone_entry(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.split_whitespace();
    let (kind, blob) = (fields.next()?, fields.next()?);
    let comment = fields.next()?;
    (kind == "ssh-ed25519" && comment.starts_with(PHONE_PREFIX)).then_some((blob, comment))
}

fn list_phone_keys_in(content: &str) -> Vec<PhoneKey> {
    content
        .lines()
        .filter_map(phone_entry)
        .map(|(blob, comment)| PhoneKey { id: blob.to_owned(), label: comment.to_owned() })
        .collect()
}

/// `None` when the key is already registered.
fn add_key_to(content: &str, public_key: &str) -> Option<String> {
    let blob = public_key.split_whitespace().nth(1)?;
    if content.lines().any(|line| line.split_whitespace().nth(1) == Some(blob)) {
        return None;
    }
    let mut next = content.to_owned();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(public_key.trim());
    next.push('\n');
    Some(next)
}

/// Only lines that `phone_entry` recognises can be removed.
fn remove_key_from(content: &str, id: &str) -> (String, bool) {
    let mut removed = false;
    let kept: String = content
        .split_inclusive('\n')
        .filter(|line| {
            let hit = phone_entry(line).is_some_and(|(blob, _)| blob == id);
            removed |= hit;
            !hit
        })
        .collect();
    (kept, removed)
}

fn edit_authorized_keys(
    dir: &Path,
    edit: impl FnOnce(&str) -> Option<String>,
) -> Result<bool, String> {
    let _guard = AUTHORIZED_KEYS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    private_dir(dir)?;
    let path = dir.join("authorized_keys");
    let current = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read authorized_keys: {error}")),
    };
    let Some(next) = edit(&current) else { return Ok(false) };
    let temporary = dir.join(format!(".authorized_keys-{}", Uuid::new_v4().simple()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(next.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not update authorized_keys: {error}"));
    }
    Ok(true)
}

/// Creates the key in a private temporary directory that is removed before
/// returning; yields (private PEM, public key line).
fn generate_key(comment: &str) -> Result<(String, String), String> {
    let dir = std::env::temp_dir().join(format!("fastade-phone-{}", Uuid::new_v4().simple()));
    private_dir(&dir)?;
    let result = (|| {
        let key = dir.join("key");
        let status = Command::new(SSH_KEYGEN)
            .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
            .arg(&key)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("Could not run ssh-keygen: {error}"))?;
        if !status.success() {
            return Err("ssh-keygen failed.".to_owned());
        }
        let private = fs::read_to_string(&key).map_err(|error| error.to_string())?;
        let public = fs::read_to_string(dir.join("key.pub")).map_err(|error| error.to_string())?;
        Ok((private, public.trim().to_owned()))
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// The address a peer on the same network would use. Connecting a UDP socket
/// sends nothing; it only selects the outbound interface.
fn lan_host() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then(|| address.to_string())
}

fn device_name() -> String {
    command_output("/usr/sbin/scutil", &["--get", "ComputerName"])
        .or_else(|| command_output("/bin/hostname", &["-s"]))
        .unwrap_or_else(|| "Mac".to_owned())
}

fn local_user() -> Option<String> {
    ["USER", "LOGNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .or_else(|| command_output("/usr/bin/id", &["-un"]))
}

fn new_key(name: &str) -> Result<NewKey, String> {
    let slug: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let suffix = Uuid::new_v4().simple().to_string();
    let comment = format!("{PHONE_PREFIX}-{}-{}", slug.trim_matches('-'), &suffix[..8]);
    let (private, public) = generate_key(&comment)?;
    Ok(NewKey { comment, private, public })
}

fn install_key(dir: &Path, key: &NewKey) -> Result<(), String> {
    edit_authorized_keys(dir, |content| add_key_to(content, &key.public)).map(|_| ())
}

fn backend_error(code: &str) -> String {
    match code {
        "auth" => "Sign in again to use the tunnel.",
        "api_unavailable" => "The server does not support phone login through the tunnel yet.",
        "key_in_use" => "The server says this key belongs to another account. Try again.",
        "key_rejected" => "The server rejected the phone key.",
        "network" => "Could not reach the server. Check your connection.",
        _ => "The server could not update the phone key.",
    }
    .to_owned()
}

#[tauri::command]
pub async fn create_phone_qr(
    tunnel: tauri::State<'_, crate::tunnel::TunnelManager>,
    name: Option<String>,
) -> Result<PhoneQr, String> {
    let forward_port = tunnel.inner().forward_port();
    let (name, username, key) = tokio::task::spawn_blocking(move || {
        let name = name
            .map(|name| name.trim().chars().take(40).collect::<String>())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(device_name);
        let username = local_user().ok_or("Could not determine the user name.")?;
        let key = new_key(&name)?;
        Ok::<_, String>((name, username, key))
    })
    .await
    .map_err(|error| error.to_string())??;

    let identity = auth::tunnel_identity();
    let (host, port, jump) = match forward_port {
        Some(port) => {
            let identity = identity.as_ref().ok_or_else(|| backend_error("auth"))?;
            let registration = auth::register_phone_key(identity, &key.public_without_comment(), &key.comment)
                .await
                .map_err(|code| backend_error(&code))?;
            let jump = PhoneJump { host: registration.jump_host, port: registration.jump_port, user: registration.jump_user };
            (registration.target_host, port, Some(jump))
        }
        None => {
            let host = lan_host().ok_or("Could not find this Mac's network address. Connect to Wi-Fi first.")?;
            (host, 22, None)
        }
    };

    let installed = {
        let key = NewKey { comment: key.comment.clone(), private: String::new(), public: key.public.clone() };
        tokio::task::spawn_blocking(move || install_key(&ssh_dir()?, &key)).await
    };
    if let Err(error) = installed.map_err(|error| error.to_string()).and_then(|result| result) {
        if let (Some(_), Some(identity)) = (&jump, &identity) {
            let _ = auth::revoke_phone_key(identity, &key.public_without_comment()).await;
        }
        return Err(error);
    }
    Ok(PhoneQr { name, host, port, username, jump, key: URL_SAFE_NO_PAD.encode(key.private) })
}

#[tauri::command]
pub async fn list_phone_keys() -> Result<Vec<PhoneKey>, String> {
    let path = ssh_dir()?.join("authorized_keys");
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(list_phone_keys_in(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read authorized_keys: {error}")),
    }
}

#[tauri::command]
pub async fn revoke_phone_key(id: String) -> Result<(), String> {
    let local_id = id.clone();
    tokio::task::spawn_blocking(move || {
        edit_authorized_keys(&ssh_dir()?, |content| {
            let (next, removed) = remove_key_from(content, &local_id);
            removed.then_some(next)
        })
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())??;
    // Without the tunnel there is no jump entry, and signing out clears it.
    let Some(identity) = auth::tunnel_identity() else { return Ok(()) };
    match auth::revoke_phone_key(&identity, &format!("ssh-ed25519 {id}")).await {
        Ok(()) => Ok(()),
        Err(code) if code == "api_unavailable" => Ok(()),
        Err(code) => Err(format!("Removed from this Mac, but not from the server: {}", backend_error(&code))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTHER: &str = "ssh-ed25519 AAAAother me@work\n";

    #[test]
    fn registration_is_deduplicated_and_keeps_other_keys() {
        let key = "ssh-ed25519 AAAAphone aissh-phone-Mac-1234abcd";
        let once = add_key_to("ssh-ed25519 AAAAother me@work", key).unwrap();
        assert_eq!(once, format!("{OTHER}{key}\n"));
        assert!(add_key_to(&once, key).is_none());
    }

    #[test]
    fn only_phone_entries_are_listed_and_revoked() {
        let content = format!("{OTHER}ssh-ed25519 AAAAphone aissh-phone-Mac-1\nssh-ed25519 AAAAold aissh-phone\n");
        let listed = list_phone_keys_in(&content);
        assert_eq!(listed.iter().map(|key| key.id.as_str()).collect::<Vec<_>>(), ["AAAAphone", "AAAAold"]);
        let (rest, removed) = remove_key_from(&content, "AAAAphone");
        assert!(removed);
        assert_eq!(rest, format!("{OTHER}ssh-ed25519 AAAAold aissh-phone\n"));
        let (same, removed) = remove_key_from(&content, "AAAAother");
        assert!(!removed);
        assert_eq!(same, content);
    }

    #[test]
    fn creating_a_qr_registers_the_key_privately_and_leaves_no_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("phone-test-{}", Uuid::new_v4().simple()));
        let dir = home.join(".ssh");
        let key = new_key("My Mac!").unwrap();
        install_key(&dir, &key).unwrap();
        let pem = key.private.clone();
        assert_eq!(key.public_without_comment().split(' ').count(), 2);
        assert!(pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        let stored = fs::read_to_string(dir.join("authorized_keys")).unwrap();
        let keys = list_phone_keys_in(&stored);
        assert_eq!(keys.len(), 1);
        assert!(keys[0].label.starts_with("aissh-phone-My-Mac-"));
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(dir.join("authorized_keys")).unwrap().permissions().mode() & 0o777, 0o600);
        let leftover = std::env::temp_dir().read_dir().unwrap().flatten().any(|entry| {
            entry.file_name().to_string_lossy().starts_with("fastade-phone-") && entry.path().join("key").exists()
        });
        assert!(!leftover);
        let _ = fs::remove_dir_all(home);
    }
}
