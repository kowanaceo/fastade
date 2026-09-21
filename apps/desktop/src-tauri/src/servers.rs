use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, process::Command};
use tauri::Manager;
use uuid::Uuid;

const KEYCHAIN_SERVICE: &str = "com.fastade.desktop.server-password";

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthMethod {
    /// Password is never stored in the JSON profile — only in the OS
    /// keychain, looked up by the server's id at connect time.
    Password,
    KeyFile {
        path: String,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedServer {
    pub(crate) id: String,
    name: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: String,
    pub(crate) auth: AuthMethod,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateServerInput {
    name: String,
    host: String,
    port: Option<u16>,
    username: String,
    auth: CreateAuthInput,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CreateAuthInput {
    Password {
        password: String,
    },
    KeyFile {
        path: String,
    },
    /// Generates a fresh ed25519 keypair for this server and uses it.
    GenerateKey,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServerInput {
    id: String,
    name: String,
    host: String,
    port: Option<u16>,
    username: String,
    auth: UpdateAuthInput,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateAuthInput {
    /// Keep whatever auth (and stored password/key) the server already has.
    Unchanged,
    Password {
        password: String,
    },
    KeyFile {
        path: String,
    },
    GenerateKey,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedServer {
    server: ManagedServer,
    /// Present only when a key was just generated — shown once so the user
    /// can copy it into the remote's `authorized_keys`.
    generated_public_key: Option<String>,
}

#[tauri::command]
pub fn list_managed_servers(app: tauri::AppHandle) -> Result<Vec<ManagedServer>, String> {
    read_servers(&servers_path(&app)?)
}

#[tauri::command]
pub fn create_managed_server(
    input: CreateServerInput,
    app: tauri::AppHandle,
) -> Result<CreatedServer, String> {
    let name = input.name.trim();
    let host = input.host.trim();
    let username = input.username.trim();
    if name.is_empty() || host.is_empty() || username.is_empty() {
        return Err("name, host, and username are required.".to_owned());
    }
    let id = Uuid::new_v4().to_string();
    let (auth, generated_public_key) = match input.auth {
        CreateAuthInput::Password { password } => {
            if password.is_empty() {
                return Err("Enter a password.".to_owned());
            }
            keychain_entry(&id)?
                .set_password(&password)
                .map_err(|error| format!("Failed to save the password to the keychain: {error}"))?;
            (AuthMethod::Password, None)
        }
        CreateAuthInput::KeyFile { path } => {
            if !PathBuf::from(&path).is_file() {
                return Err(format!("Could not find the key file: {path}"));
            }
            (AuthMethod::KeyFile { path }, None)
        }
        CreateAuthInput::GenerateKey => {
            let (private_path, public_key) = generate_keypair(&app, &id)?;
            (AuthMethod::KeyFile { path: private_path }, Some(public_key))
        }
    };
    let server = ManagedServer {
        id,
        name: name.to_owned(),
        host: host.to_owned(),
        port: input.port.unwrap_or(22),
        username: username.to_owned(),
        auth,
    };
    let path = servers_path(&app)?;
    let mut servers = read_servers(&path)?;
    servers.push(server.clone());
    write_servers(&path, &servers)?;
    Ok(CreatedServer {
        server,
        generated_public_key,
    })
}

#[tauri::command]
pub fn update_managed_server(
    input: UpdateServerInput,
    app: tauri::AppHandle,
) -> Result<CreatedServer, String> {
    let name = input.name.trim();
    let host = input.host.trim();
    let username = input.username.trim();
    if name.is_empty() || host.is_empty() || username.is_empty() {
        return Err("name, host, and username are required.".to_owned());
    }
    let path = servers_path(&app)?;
    let mut servers = read_servers(&path)?;
    let index = servers
        .iter()
        .position(|server| server.id == input.id)
        .ok_or_else(|| "Could not find the server.".to_owned())?;
    let previous_auth = servers[index].auth.clone();

    let (auth, generated_public_key) = match input.auth {
        UpdateAuthInput::Unchanged => (previous_auth.clone(), None),
        UpdateAuthInput::Password { password } => {
            if password.is_empty() {
                return Err("Enter a password.".to_owned());
            }
            keychain_entry(&input.id)?
                .set_password(&password)
                .map_err(|error| format!("Failed to save the password to the keychain: {error}"))?;
            (AuthMethod::Password, None)
        }
        UpdateAuthInput::KeyFile { path } => {
            if !PathBuf::from(&path).is_file() {
                return Err(format!("Could not find the key file: {path}"));
            }
            (AuthMethod::KeyFile { path }, None)
        }
        UpdateAuthInput::GenerateKey => {
            let (private_path, public_key) = generate_keypair(&app, &input.id)?;
            (AuthMethod::KeyFile { path: private_path }, Some(public_key))
        }
    };
    forget_stale_auth(&app, &input.id, &previous_auth, &auth);

    let server = ManagedServer {
        id: input.id,
        name: name.to_owned(),
        host: host.to_owned(),
        port: input.port.unwrap_or(22),
        username: username.to_owned(),
        auth,
    };
    servers[index] = server.clone();
    write_servers(&path, &servers)?;
    Ok(CreatedServer {
        server,
        generated_public_key,
    })
}

/// When auth changes away from a password, the old keychain entry is
/// useless; away from a self-generated key file, the old key files are too
/// (never touches a path the user picked themselves, only ones under our
/// own server-keys directory).
fn forget_stale_auth(app: &tauri::AppHandle, id: &str, previous: &AuthMethod, next: &AuthMethod) {
    if matches!(previous, AuthMethod::Password) && !matches!(next, AuthMethod::Password) {
        if let Ok(entry) = keychain_entry(id) {
            let _ = entry.delete_credential();
        }
    }
    if let AuthMethod::KeyFile { path } = previous {
        let still_used =
            matches!(next, AuthMethod::KeyFile { path: next_path } if next_path == path);
        let generated_dir = server_key_dir(app).ok().map(|dir| dir.join(id));
        let is_ours = generated_dir
            .as_ref()
            .is_some_and(|dir| PathBuf::from(path).starts_with(dir));
        if !still_used && is_ours {
            if let Some(dir) = generated_dir {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }
}

#[tauri::command]
pub fn delete_managed_server(id: String, app: tauri::AppHandle) -> Result<(), String> {
    let path = servers_path(&app)?;
    let mut servers = read_servers(&path)?;
    let previous_len = servers.len();
    servers.retain(|server| server.id != id);
    if servers.len() == previous_len {
        return Err("Could not find the server.".to_owned());
    }
    write_servers(&path, &servers)?;
    if let Ok(entry) = keychain_entry(&id) {
        let _ = entry.delete_credential();
    }
    if let Some(directory) = server_key_dir(&app).ok().map(|dir| dir.join(&id)) {
        let _ = fs::remove_dir_all(directory);
    }
    Ok(())
}

/// Looks up a managed server's connection info by id, for building the
/// actual `ssh` invocation. Not exposed to the frontend — the password (if
/// any) never leaves the Rust process.
pub fn resolve_managed_server(
    app: &tauri::AppHandle,
    id: &str,
) -> Result<(ManagedServer, Option<String>), String> {
    let server = read_servers(&servers_path(app)?)?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or_else(|| "Could not find the server.".to_owned())?;
    // A stored password becoming unreadable (a platform keychain quirk, or
    // the entry never having been created) shouldn't block connecting — ssh
    // just falls back to its own interactive password prompt in the PTY.
    let password = match &server.auth {
        AuthMethod::Password => keychain_entry(id)
            .ok()
            .and_then(|entry| entry.get_password().ok()),
        AuthMethod::KeyFile { .. } => None,
    };
    Ok((server, password))
}

fn keychain_entry(id: &str) -> Result<Entry, String> {
    Entry::new(KEYCHAIN_SERVICE, id).map_err(|error| format!("Keychain access failed: {error}"))
}

fn generate_keypair(app: &tauri::AppHandle, id: &str) -> Result<(String, String), String> {
    let directory = server_key_dir(app)?.join(id);
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let private_path = directory.join("id_ed25519");
    let output = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", "fastade", "-f"])
        .arg(&private_path)
        .output()
        .map_err(|error| format!("Failed to run ssh-keygen: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to generate key: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let public_key = fs::read_to_string(directory.join("id_ed25519.pub"))
        .map_err(|error| format!("Could not read the public key: {error}"))?
        .trim()
        .to_owned();
    Ok((private_path.to_string_lossy().into_owned(), public_key))
}

fn server_key_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("server-keys"))
        .map_err(|error| error.to_string())
}

fn servers_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("managed-servers.json"))
        .map_err(|error| error.to_string())
}

fn read_servers(path: &PathBuf) -> Result<Vec<ManagedServer>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("managed server data is invalid: {error}"))
}

fn write_servers(path: &PathBuf, servers: &[ManagedServer]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = serde_json::to_string_pretty(servers).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())
}

#[cfg(test)]
mod keychain_smoke {
    use super::keychain_entry;
    #[test]
    #[ignore = "touches the real OS keychain; run manually"]
    fn roundtrip() {
        let entry = keychain_entry("fastade-smoke-test").unwrap();
        entry.set_password("hunter2").unwrap();
        assert_eq!(entry.get_password().unwrap(), "hunter2");
        entry.delete_credential().unwrap();
    }
}
