use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
use url::Url;
use uuid::Uuid;

const BACKEND_BASE_URL: &str = "https://backend.kowanas.com/aisshapi";
const GOOGLE_CLIENT_ID: &str = env!("FASTADE_GOOGLE_CLIENT_ID");
const GOOGLE_CLIENT_SECRET: &str = env!("FASTADE_GOOGLE_CLIENT_SECRET");
const KEYCHAIN_SERVICE: &str = "com.fastade.desktop.auth";
const KEYCHAIN_SESSION_USER: &str = "backend-session";
const KEYCHAIN_DEVICE_USER: &str = "device-id";
static REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    #[serde(deserialize_with = "string_id")]
    id: String,
    name: String,
    email: String,
    picture: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    configured: bool,
    user: Option<AuthUser>,
    device_id: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendSession {
    access_token: String,
    access_token_expires_at: serde_json::Value,
    refresh_token: String,
    refresh_token_expires_at: serde_json::Value,
    token_type: String,
    user: AuthUser,
    device: BackendDevice,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackendDevice {
    device_id: String,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshedTokens {
    access_token: String,
    access_token_expires_at: serde_json::Value,
    refresh_token: String,
    refresh_token_expires_at: serde_json::Value,
    token_type: String,
}

fn string_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(value) => Ok(value),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        _ => Err(serde::de::Error::custom("user id must be a string or number")),
    }
}

#[tauri::command]
pub fn google_auth_status() -> AuthStatus {
    let session = read_session();
    AuthStatus {
        configured: !GOOGLE_CLIENT_ID.is_empty(),
        user: session.as_ref().map(|value| value.user.clone()),
        device_id: session.map(|value| value.device.device_id),
    }
}

#[tauri::command]
pub async fn google_sign_in() -> Result<AuthUser, String> {
    if GOOGLE_CLIENT_ID.is_empty() {
        return Err("Google OAuth is not configured for this build.".to_owned());
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("Could not start the login callback: {error}"))?;
    let callback_address = listener.local_addr().map_err(|error| error.to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{}", callback_address.port());
    let state = Uuid::new_v4().simple().to_string();
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let mut authorization_url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
        .map_err(|error| error.to_string())?;
    authorization_url
        .query_pairs_mut()
        .append_pair("client_id", GOOGLE_CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    open_system_browser(authorization_url.as_str())?;
    let callback_state = state.clone();
    let code =
        tauri::async_runtime::spawn_blocking(move || receive_callback(listener, &callback_state))
            .await
            .map_err(|error| format!("Login callback stopped unexpectedly: {error}"))??;

    let mut form = vec![
        ("client_id", GOOGLE_CLIENT_ID.to_owned()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code".to_owned()),
        ("redirect_uri", redirect_uri),
    ];
    if !GOOGLE_CLIENT_SECRET.is_empty() {
        form.push(("client_secret", GOOGLE_CLIENT_SECRET.to_owned()));
    }
    let response = reqwest::Client::new()
        .post("https://oauth2.googleapis.com/token")
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("Could not reach Google: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&detail)
            .ok()
            .and_then(|value| {
                let code = value.get("error")?.as_str()?;
                let description = value
                    .get("error_description")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                Some(format!("{code}: {description}"))
            })
            .unwrap_or_else(|| detail.trim().to_owned());
        return Err(if detail.is_empty() {
            format!("Google rejected the login request ({status}).")
        } else {
            format!("Google rejected the login request ({status}): {detail}")
        });
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|error| format!("Google returned an invalid login response: {error}"))?;
    // Google proves the identity to the fastade backend. The backend verifies
    // the token and issues the only credentials used for sync; keeping just a
    // decoded Google profile locally would look signed in without actually
    // authorising any backend request.
    let device_id = device_id()?;
    let response = reqwest::Client::new()
        .post(format!("{BACKEND_BASE_URL}/auth/google"))
        .json(&serde_json::json!({
            "idToken": token.id_token,
            "deviceId": device_id,
            "platform": std::env::consts::OS,
        }))
        .send()
        .await
        .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    let session: BackendSession = response_json(response, "Fastade rejected the login").await?;
    let user = session.user.clone();
    write_session(&session)?;
    Ok(user)
}

#[tauri::command]
pub async fn google_sign_out() -> Result<(), String> {
    // Local sign-out must always succeed even if the network is unavailable.
    // The remote session will then expire naturally; when reachable, revoke it
    // first so the refresh token cannot be reused.
    if let Some(session) = read_session() {
        let _ = reqwest::Client::new()
            .post(format!("{BACKEND_BASE_URL}/auth/logout"))
            .bearer_auth(session.access_token)
            .json(&serde_json::json!({ "allDevices": false }))
            .send()
            .await;
    }
    let entry = session_entry()?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not remove the saved login: {error}")),
    }
}

#[tauri::command]
pub async fn sync_snapshot() -> Result<serde_json::Value, String> {
    authenticated_get("/sync/snapshot").await
}

#[tauri::command]
pub async fn sync_changes(
    cursor: u64,
    limit: u16,
    wait_seconds: u8,
) -> Result<serde_json::Value, String> {
    let limit = limit.clamp(1, 1000);
    let wait_seconds = wait_seconds.min(30);
    authenticated_get(&format!(
        "/sync/changes?cursor={cursor}&limit={limit}&waitSeconds={wait_seconds}"
    ))
    .await
}

#[tauri::command]
pub async fn sync_push(changes: Vec<serde_json::Value>) -> Result<serde_json::Value, String> {
    authenticated_post("/sync/push", &serde_json::json!({ "changes": changes })).await
}

/// A host's usage snapshot, stored by the account so other devices can show
/// it. One row per (agent, host) that each upload replaces; nothing is kept
/// as history, and the sync change log is deliberately not involved.
#[tauri::command]
pub async fn put_usage_snapshot(snapshot: serde_json::Value) -> Result<(), String> {
    let part = |key: &str| {
        snapshot
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| {
                !value.is_empty()
                    && value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .map(str::to_owned)
            .ok_or_else(|| format!("invalid usage snapshot {key}"))
    };
    let (agent, host) = (part("agentId")?, part("hostId")?);
    authenticated_put(&format!("/usage/snapshots/{agent}/{host}"), &snapshot).await
}

#[tauri::command]
pub async fn list_usage_snapshots() -> Result<serde_json::Value, String> {
    authenticated_get("/usage/snapshots").await
}

fn receive_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err("Google login timed out. Please try again.".to_owned());
            }
            Err(error) => return Err(format!("Could not receive the Google callback: {error}")),
        }
    };
    finish_callback(&mut stream, expected_state)
}

fn finish_callback(stream: &mut TcpStream, expected_state: &str) -> Result<String, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    let mut request = [0_u8; 8192];
    let length = stream
        .read(&mut request)
        .map_err(|error| error.to_string())?;
    let request = String::from_utf8_lossy(&request[..length]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "Google returned an invalid callback.".to_owned())?;
    let callback = Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| "Google returned an invalid callback URL.".to_owned())?;
    let parameters: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
    let result = if let Some(error) = parameters.get("error") {
        Err(format!("Google login was not completed: {error}"))
    } else if parameters.get("state").map(String::as_str) != Some(expected_state) {
        Err("Google login state did not match. Please try again.".to_owned())
    } else {
        parameters
            .get("code")
            .cloned()
            .ok_or_else(|| "Google did not return an authorization code.".to_owned())
    };
    let (status, message) = if result.is_ok() {
        (
            "200 OK",
            "Google login complete. You can close this tab and return to fastade.",
        )
    } else {
        (
            "400 Bad Request",
            "Google login could not be completed. Return to fastade and try again.",
        )
    };
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>fastade</title><style>body{{font:16px system-ui;background:#101612;color:#dce5df;display:grid;place-items:center;height:100vh;margin:0}}div{{text-align:center}}b{{color:#8eefaa}}</style><div><b>fastade</b><p>{message}</p></div>");
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
    result
}

fn read_session() -> Option<BackendSession> {
    session_entry()
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .and_then(|value| serde_json::from_str(&value).ok())
}

/// Remote MCP access is available only while this device has a Fastade
/// account session. The credential itself stays in the OS keychain; callers
/// use this only as an enablement check and must never copy it to a remote
/// shell or config file.
pub(crate) fn is_signed_in() -> bool {
    read_session().is_some()
}

fn write_session(session: &BackendSession) -> Result<(), String> {
    let value = serde_json::to_string(session).map_err(|error| error.to_string())?;
    session_entry()?
        .set_password(&value)
        .map_err(|error| format!("Could not save the login securely: {error}"))
}

fn session_entry() -> Result<Entry, String> {
    Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_SESSION_USER)
        .map_err(|error| format!("Keychain access failed: {error}"))
}

fn device_id() -> Result<String, String> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_DEVICE_USER)
        .map_err(|error| format!("Keychain access failed: {error}"))?;
    if let Ok(value) = entry.get_password() {
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
    let value = Uuid::new_v4().to_string();
    entry
        .set_password(&value)
        .map_err(|error| format!("Could not save the device identity: {error}"))?;
    Ok(value)
}

async fn authenticated_get(path: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let mut session = read_session().ok_or_else(|| "Sign in to sync sessions.".to_owned())?;
    let mut response = client
        .get(format!("{BACKEND_BASE_URL}{path}"))
        .bearer_auth(&session.access_token)
        .send()
        .await
        .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        session = refresh_session_if_needed(&client, session).await?;
        response = client
            .get(format!("{BACKEND_BASE_URL}{path}"))
            .bearer_auth(&session.access_token)
            .send()
            .await
            .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    }
    response_json(response, "Fastade sync failed").await
}

async fn authenticated_post(
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let mut session = read_session().ok_or_else(|| "Sign in to sync sessions.".to_owned())?;
    let mut response = client
        .post(format!("{BACKEND_BASE_URL}{path}"))
        .bearer_auth(&session.access_token)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        session = refresh_session_if_needed(&client, session).await?;
        response = client
            .post(format!("{BACKEND_BASE_URL}{path}"))
            .bearer_auth(&session.access_token)
            .json(body)
            .send()
            .await
            .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    }
    response_json(response, "Fastade sync failed").await
}

async fn authenticated_put(path: &str, body: &serde_json::Value) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut session = read_session().ok_or_else(|| "Sign in to sync usage.".to_owned())?;
    let mut response = client
        .put(format!("{BACKEND_BASE_URL}{path}"))
        .bearer_auth(&session.access_token)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        session = refresh_session_if_needed(&client, session).await?;
        response = client
            .put(format!("{BACKEND_BASE_URL}{path}"))
            .bearer_auth(&session.access_token)
            .json(body)
            .send()
            .await
            .map_err(|error| format!("Could not reach the fastade server: {error}"))?;
    }
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let detail = response.text().await.unwrap_or_default();
    Err(if detail.trim().is_empty() {
        format!("Fastade usage upload failed ({status})")
    } else {
        format!("Fastade usage upload failed ({status}): {detail}")
    })
}

async fn refresh_session_if_needed(
    client: &reqwest::Client,
    mut session: BackendSession,
) -> Result<BackendSession, String> {
    let lock = REFRESH_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;
    // A snapshot long-poll and a local push can discover expiry together.
    // The first rotates the refresh token; followers must reuse that newly
    // persisted session instead of submitting the now-consumed old token.
    if let Some(current) = read_session() {
        if current.access_token != session.access_token {
            return Ok(current);
        }
    }
    let response = client
        .post(format!("{BACKEND_BASE_URL}/auth/refresh"))
        .json(&serde_json::json!({ "refreshToken": session.refresh_token }))
        .send()
        .await
        .map_err(|error| format!("Could not refresh the fastade login: {error}"))?;
    let refreshed: RefreshedTokens = response_json(response, "Fastade login expired").await?;
    session.access_token = refreshed.access_token;
    session.access_token_expires_at = refreshed.access_token_expires_at;
    session.refresh_token = refreshed.refresh_token;
    session.refresh_token_expires_at = refreshed.refresh_token_expires_at;
    session.token_type = refreshed.token_type;
    // Refresh tokens rotate. Persist the replacement before retrying the API
    // request so an app crash cannot leave only the already-consumed token.
    write_session(&session)?;
    Ok(session)
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    context: &str,
) -> Result<T, String> {
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|error| format!("{context}: invalid response ({error})"));
    }
    let detail = response.text().await.unwrap_or_default();
    Err(if detail.trim().is_empty() {
        format!("{context} ({status})")
    } else {
        format!("{context} ({status}): {detail}")
    })
}

fn open_system_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open the system browser: {error}"))
}
