//! One app-owned reverse tunnel. Configuration is local; account credentials
//! stay in auth.rs and only the assigned port/heartbeat reach the backend.
use crate::auth::{self, TunnelIdentity, TunnelRegistration};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::{Emitter, Manager};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    net::TcpStream,
    process::{Child, ChildStdin, Command},
    sync::{watch, Mutex},
    time::{sleep, timeout, Instant},
};
use uuid::Uuid;

const HEARTBEAT: Duration = Duration::from_secs(30);
const REPORT_TIMEOUT: Duration = Duration::from_secs(9);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelSettings {
    backend_host: String,
    /// None preserves the SSH alias's configured port.
    ssh_port: Option<u16>,
    identity_file: String,
}

impl Default for TunnelSettings {
    fn default() -> Self {
        Self {
            backend_host: "backend".into(),
            ssh_port: None,
            identity_file: "~/.ssh/fastade-tunnel".into(),
        }
    }
}

impl TunnelSettings {
    fn validated(mut self) -> Result<Self, String> {
        self.backend_host = self.backend_host.trim().to_owned();
        if self.backend_host.is_empty()
            || self.backend_host.starts_with('-')
            || self.backend_host.len() > 255
            || !self
                .backend_host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-@:[]".contains(&c))
            || self.ssh_port == Some(0)
        {
            return Err("settings".into());
        }
        self.identity_file = self.identity_file.trim().to_owned();
        if let Some(relative) = self.identity_file.strip_prefix("~/") {
            self.identity_file = dirs::home_dir()
                .ok_or("key")?
                .join(relative)
                .to_string_lossy()
                .into();
        }
        let path = Path::new(&self.identity_file);
        if !path.is_absolute() || !path.is_file() || self.identity_file.contains(['\n', '\r', '\0'])
        {
            return Err("key".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(path).map_err(|_| "key")?.permissions().mode() & 0o077 != 0 {
                return Err("key_permissions".into());
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    supported: bool,
    enabled: bool,
    phase: String,
    settings: TunnelSettings,
    port: Option<u16>,
    error: Option<String>,
    sync_error: Option<String>,
    retry_in_seconds: Option<u64>,
}

#[derive(Clone)]
struct Request {
    settings: TunnelSettings,
    identity: TunnelIdentity,
}

#[derive(Clone)]
pub struct TunnelManager {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    app: Option<tauri::AppHandle>,
    desired: watch::Sender<Option<Request>>,
    status: watch::Sender<TunnelStatus>,
    operation: Arc<Mutex<()>>,
    quitting: AtomicBool,
    exit_ready: AtomicBool,
}

impl TunnelManager {
    pub fn start(app: tauri::AppHandle) -> Result<Self, String> {
        let path = app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("tunnel-settings.json");
        let settings = if path.exists() {
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).unwrap_or_default()
        } else {
            TunnelSettings::default()
        };
        let (desired, receiver) = watch::channel(None);
        let (status, _) = watch::channel(TunnelStatus {
            supported: cfg!(target_os = "macos"),
            enabled: false,
            phase: "off".into(),
            settings,
            port: None,
            error: None,
            sync_error: None,
            retry_in_seconds: None,
        });
        let manager = Self {
            inner: Arc::new(Inner {
                path,
                app: Some(app),
                desired,
                status,
                operation: Arc::new(Mutex::new(())),
                quitting: AtomicBool::new(false),
                exit_ready: AtomicBool::new(false),
            }),
        };
        let worker = manager.clone();
        tauri::async_runtime::spawn(async move {
            worker.supervise(receiver).await;
        });
        Ok(manager)
    }

    fn status(&self) -> TunnelStatus {
        self.inner.status.borrow().clone()
    }

    /// The backend port forwarded to this Mac's sshd. None unless the tunnel
    /// is connected.
    pub fn forward_port(&self) -> Option<u16> {
        let status = self.status();
        status.port.filter(|_| status.phase == "connected")
    }

    fn update(&self, change: impl FnOnce(&mut TunnelStatus)) {
        self.inner.status.send_modify(change);
        if let Some(app) = &self.inner.app {
            let _ = app.emit("tunnel-changed", self.status());
        }
    }

    pub fn begin_shutdown(&self) -> bool {
        !self.inner.quitting.swap(true, Ordering::SeqCst)
    }

    pub fn exit_ready(&self) -> bool {
        self.inner.exit_ready.load(Ordering::SeqCst)
    }

    pub fn finish_shutdown(&self) {
        self.inner.exit_ready.store(true, Ordering::SeqCst);
    }

    pub async fn stop(&self) {
        let _guard = self.inner.operation.lock().await;
        self.stop_locked().await;
    }

    /// Keep enable/save serialized with the complete credential change, not
    /// just the stop. Another window must not re-enable between stop and logout.
    pub async fn stop_for_account_change(&self) -> tokio::sync::OwnedMutexGuard<()> {
        let guard = self.inner.operation.clone().lock_owned().await;
        self.stop_locked().await;
        guard
    }

    async fn stop_locked(&self) {
        let mut status = self.inner.status.subscribe();
        let active = self.inner.desired.borrow().is_some();
        if !active {
            self.update(|s| {
                s.enabled = false;
                s.phase = "off".into();
                s.error = None;
                s.retry_in_seconds = None;
            });
            return;
        }
        self.update(|s| {
            s.enabled = false;
            s.phase = "stopping".into();
            s.retry_in_seconds = None;
        });
        self.inner.desired.send_replace(None);
        // The supervisor first drops/kills both child processes, then reports
        // off with a bounded request. No live tunnel remains while waiting.
        loop {
            if matches!(status.borrow_and_update().phase.as_str(), "off" | "failed") {
                break;
            }
            if status.changed().await.is_err() {
                break;
            }
        }
    }

    async fn supervise(&self, mut desired: watch::Receiver<Option<Request>>) {
        loop {
            let request = desired.borrow_and_update().clone();
            let Some(request) = request else {
                // ON can be followed by OFF before this worker even observes
                // ON. Acknowledge that cancellation so stop never waits forever.
                self.update(|s| {
                    s.enabled = false;
                    s.phase = "off".into();
                    s.retry_in_seconds = None;
                });
                if desired.changed().await.is_err() {
                    break;
                }
                continue;
            };
            let reporter = AccountReporter {
                identity: request.identity,
                hostname: sysinfo::System::host_name()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Mac".into())
                    .chars()
                    .take(200)
                    .collect(),
            };
            let programs = Programs::default();
            let outcome = tokio::select! {
                biased;
                _ = desired.changed() => None,
                result = run_enabled(self, &request.settings, &reporter, &programs) => Some(result),
            };
            // Cancellation drops the SSH lifetime pipe and sleep assertion
            // before publishing off, including while connection starts.
            let off = report(&reporter, false).await;
            match outcome {
                Some(Err(error)) => {
                    self.inner.desired.send_replace(None);
                    self.update(|s| {
                        s.enabled = false;
                        s.phase = "failed".into();
                        s.error = Some(error);
                        s.sync_error = off.err();
                        s.retry_in_seconds = None;
                    });
                }
                _ => self.update(|s| {
                    s.enabled = false;
                    s.phase = "off".into();
                    s.error = None;
                    s.sync_error = off.err();
                    s.retry_in_seconds = None;
                }),
            }
        }
    }
}

#[tauri::command]
pub fn tunnel_status(manager: tauri::State<'_, TunnelManager>) -> TunnelStatus {
    manager.status()
}

#[tauri::command]
pub async fn save_tunnel_settings(
    settings: TunnelSettings,
    manager: tauri::State<'_, TunnelManager>,
) -> Result<TunnelStatus, String> {
    let _guard = manager.inner.operation.lock().await;
    if manager.inner.desired.borrow().is_some() || manager.status().phase == "stopping" {
        return Err("busy".into());
    }
    ensure_identity(&settings).await?;
    let settings = settings.validated()?;
    save_settings(&manager.inner.path, &settings)?;
    manager.update(|s| {
        s.settings = settings;
        s.error = None;
        s.phase = "off".into();
    });
    Ok(manager.status())
}

#[tauri::command]
pub async fn set_tunnel_enabled(
    enabled: bool,
    manager: tauri::State<'_, TunnelManager>,
) -> Result<TunnelStatus, String> {
    let _guard = manager.inner.operation.lock().await;
    if !enabled {
        manager.stop_locked().await;
        return Ok(manager.status());
    }
    if manager.inner.quitting.load(Ordering::SeqCst) {
        return Err("stopping".into());
    }
    if !cfg!(target_os = "macos") {
        return Err("unsupported".into());
    }
    if manager.inner.desired.borrow().is_some() {
        return Ok(manager.status());
    }
    let identity = auth::tunnel_identity().ok_or("auth")?;
    let settings = manager.status().settings;
    ensure_identity(&settings).await?;
    let settings = settings.validated()?;
    ensure_sshd(22).await?;
    manager.update(|s| {
        s.enabled = true;
        s.phase = "connecting".into();
        s.error = None;
        s.sync_error = None;
        s.retry_in_seconds = None;
    });
    manager
        .inner
        .desired
        .send_replace(Some(Request { settings, identity }));
    Ok(manager.status())
}

fn save_settings(path: &Path, settings: &TunnelSettings) -> Result<(), String> {
    let parent = path.parent().ok_or("settings_save")?;
    fs::create_dir_all(parent).map_err(|_| "settings_save")?;
    let temporary = parent.join(format!(".tunnel-{}.json", Uuid::new_v4().simple()));
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
        file.write_all(&serde_json::to_vec_pretty(settings)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_: std::io::Error| "settings_save".to_owned())
}

trait Reporter: Sync {
    fn current(&self) -> bool;
    fn report(&self, on: bool) -> impl Future<Output = Result<TunnelRegistration, String>> + Send;
    /// Upload the public half of the private key at `identity_file`.
    fn register_key(&self, identity_file: &str) -> impl Future<Output = Result<(), String>> + Send;
}

struct AccountReporter {
    identity: TunnelIdentity,
    hostname: String,
}
impl Reporter for AccountReporter {
    fn current(&self) -> bool {
        auth::tunnel_identity().as_ref() == Some(&self.identity)
    }
    async fn report(&self, on: bool) -> Result<TunnelRegistration, String> {
        auth::report_tunnel(&self.identity, on, &self.hostname).await
    }
    async fn register_key(&self, identity_file: &str) -> Result<(), String> {
        let public_key = public_key_of(Path::new(identity_file)).await?;
        auth::register_tunnel_key(&self.identity, &public_key).await
    }
}

async fn register_key(reporter: &impl Reporter, identity_file: &str) -> Result<(), String> {
    timeout(REPORT_TIMEOUT, reporter.register_key(identity_file))
        .await
        .map_err(|_| "network".to_owned())?
}

pub(crate) const SSH_KEYGEN: &str = "/usr/bin/ssh-keygen";

fn identity_path(settings: &TunnelSettings) -> Result<PathBuf, String> {
    let file = settings.identity_file.trim();
    let path = match file.strip_prefix("~/") {
        Some(relative) => dirs::home_dir().ok_or("key")?.join(relative),
        None => PathBuf::from(file),
    };
    if !path.is_absolute() || file.contains(['\n', '\r', '\0']) {
        return Err("key".into());
    }
    Ok(path)
}

/// Create the tunnel's dedicated key when it does not exist yet. An existing
/// file is never touched, so a user-chosen key keeps working.
async fn ensure_identity(settings: &TunnelSettings) -> Result<(), String> {
    let path = identity_path(settings)?;
    if path.exists() {
        return Ok(());
    }
    let parent = path.parent().ok_or("key")?;
    if !parent.is_dir() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent).map_err(|_| "key_generate")?;
    }
    let status = child_command(Path::new(SSH_KEYGEN))
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "fastade-tunnel", "-f"])
        .arg(&path)
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|_| "key_generate")?;
    if !status.success() {
        return Err("key_generate".into());
    }
    Ok(())
}

/// `ssh-ed25519 <base64>` derived from the private key, so a missing `.pub`
/// file is not a problem. Comments are dropped because the backend rejects them.
async fn public_key_of(identity: &Path) -> Result<String, String> {
    let output = child_command(Path::new(SSH_KEYGEN))
        .args(["-y", "-f"])
        .arg(identity)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|_| "key")?;
    if !output.status.success() {
        // Encrypted keys cannot be read non-interactively.
        return Err("key".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some("ssh-ed25519"), Some(data)) => Ok(format!("ssh-ed25519 {data}")),
        _ => Err("key_type".into()),
    }
}

async fn report(reporter: &impl Reporter, on: bool) -> Result<TunnelRegistration, String> {
    timeout(REPORT_TIMEOUT, reporter.report(on))
        .await
        .map_err(|_| "network".to_owned())?
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(6)).min(60))
}

async fn run_enabled(
    manager: &TunnelManager,
    settings: &TunnelSettings,
    reporter: &impl Reporter,
    programs: &Programs,
) -> Result<(), String> {
    if !reporter.current() {
        return Err("auth".into());
    }
    // Keep the Mac awake during connection attempts and backoff too. The
    // assertion also ends automatically if the owning app is force-quit.
    let mut awake = prevent_sleep(programs)?;
    tokio::select! {
        result = reconnect(manager, settings, reporter, programs) => result,
        _ = awake.wait() => Err("sleep_prevention".into()),
    }
}

async fn reconnect(
    manager: &TunnelManager,
    settings: &TunnelSettings,
    reporter: &impl Reporter,
    programs: &Programs,
) -> Result<(), String> {
    let mut attempts = 0;
    loop {
        if !reporter.current() {
            return Err("auth".into());
        }
        manager.update(|s| {
            s.enabled = true;
            s.phase = if attempts == 0 {
                "connecting"
            } else {
                "reconnecting"
            }
            .into();
            s.retry_in_seconds = None;
        });
        let connected_at = Instant::now();
        let result = run_connection(manager, settings, reporter, programs).await;
        if matches!(
            result.as_ref().err().map(String::as_str),
            Some(
                "auth"
                    | "host_key"
                    | "ssh_auth"
                    | "key"
                    | "key_permissions"
                    | "key_type"
                    | "key_in_use"
                    | "key_rejected"
                    | "settings"
                    | "api_unavailable"
            )
        ) {
            return result;
        }
        // A disconnected process has already been killed/dropped. Mark the
        // backend off before waiting or making another connection attempt.
        let off = report(reporter, false).await;
        if off.as_ref().err().map(String::as_str) == Some("auth") {
            return Err("auth".into());
        }
        if connected_at.elapsed() >= Duration::from_secs(60) {
            attempts = 0;
        }
        let delay = retry_delay(attempts);
        attempts = attempts.saturating_add(1);
        manager.update(|s| {
            s.phase = "reconnecting".into();
            s.error = result.err();
            s.sync_error = off.err();
            s.retry_in_seconds = Some(delay.as_secs());
        });
        sleep(delay).await;
    }
}

async fn run_connection(
    manager: &TunnelManager,
    settings: &TunnelSettings,
    reporter: &impl Reporter,
    programs: &Programs,
) -> Result<(), String> {
    let settings = &settings.clone().validated()?;
    ensure_sshd(programs.local_port).await?;
    // The key must be on the backend before ssh -R can authenticate. It is
    // idempotent and also covers a re-login, which clears the backend copy.
    register_key(reporter, &settings.identity_file).await?;
    // Allocate with on=false so a phone never sees an unverified connection.
    let registration = report(reporter, false).await?;
    let port = registration.port;
    manager.update(|s| {
        s.port = Some(port);
    });
    let mut processes = SessionProcesses::connect(settings, port, programs).await?;
    let registered = report(reporter, true).await?;
    if registered.port != port {
        return Err("port_changed".into());
    }
    manager.update(|s| {
        s.phase = "connected".into();
        s.error = None;
        s.sync_error = None;
    });
    loop {
        tokio::select! {
            _ = processes.ssh.wait() => return Err(processes.error().await),
            _ = sleep(HEARTBEAT) => {
                if !reporter.current() { return Err("auth".into()); }
                ensure_sshd(programs.local_port).await?;
                match report(reporter, true).await {
                    Ok(reply) if reply.port == port => manager.update(|s| { s.sync_error = None; }),
                    Ok(_) => return Err("port_changed".into()),
                    Err(error) if error == "auth" => return Err(error),
                    Err(error) => manager.update(|s| { s.sync_error = Some(error); }),
                }
            }
        }
    }
}

async fn ensure_sshd(port: u16) -> Result<(), String> {
    timeout(Duration::from_secs(2), async {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|_| "sshd")?;
        let mut reader = BufReader::new(stream.take(4096));
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await.map_err(|_| "sshd")? == 0 {
                return Err("sshd".into());
            }
            if line.starts_with("SSH-") {
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| "sshd".to_owned())?
}

#[derive(Clone)]
struct Programs {
    ssh: PathBuf,
    caffeinate: PathBuf,
    local_port: u16,
}
impl Default for Programs {
    fn default() -> Self {
        Self {
            ssh: "/usr/bin/ssh".into(),
            caffeinate: "/usr/bin/caffeinate".into(),
            local_port: 22,
        }
    }
}

struct ControlDirectory(PathBuf);
impl ControlDirectory {
    fn create() -> Result<Self, String> {
        let path =
            std::env::temp_dir().join(format!("ft-{}", &Uuid::new_v4().simple().to_string()[..16]));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|_| "ssh_start")?;
        Ok(Self(path))
    }
    fn socket(&self) -> PathBuf {
        self.0.join("s")
    }
}
impl Drop for ControlDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.socket());
        let _ = fs::remove_dir(&self.0);
    }
}

struct SessionProcesses {
    ssh: Child,
    // Keep this pipe outside Child: Child::wait closes its own stdin. EOF is
    // the guard's lifetime signal, including when the app is force-killed.
    _lease: Option<ChildStdin>,
    _directory: ControlDirectory,
}

// Static code only; the executable and all SSH arguments are passed through
// "$@" as literal arguments. No user value is interpolated into shell code.
// A blocking pipe reader signals the guard on EOF. This avoids relying on
// shell-specific read timeout statuses (macOS ships an older shell).
const SSH_LIFETIME_GUARD: &str = r#"
guard_pid=$$
ssh_pid=
reader_pid=
cleanup() {
  if [ -n "$reader_pid" ]; then
    kill "$reader_pid" 2>/dev/null
    wait "$reader_pid" 2>/dev/null
  fi
  if [ -n "$ssh_pid" ]; then
    kill "$ssh_pid" 2>/dev/null
    wait "$ssh_pid" 2>/dev/null
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM USR1
exec 3<&0
(
  trap - EXIT HUP INT TERM USR1
  while IFS= read -r line; do :; done
  kill -USR1 "$guard_pid" 2>/dev/null
) <&3 3<&- &
reader_pid=$!
"$@" < /dev/null 3<&- &
ssh_pid=$!
exec 3<&-
wait "$ssh_pid"
result=$?
ssh_pid=
exit "$result"
"#;

fn owned_ssh(program: &Path, arguments: Vec<String>) -> Result<Child, String> {
    Command::new("/bin/sh")
        .args(["-c", SSH_LIFETIME_GUARD, "fastade-ssh-guard"])
        .arg(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // The guard must live long enough to kill and reap SSH on stdin EOF.
        // Killing the guard itself would orphan its SSH child.
        .kill_on_drop(false)
        .spawn()
        .map_err(|_| "ssh_start".into())
}

fn prevent_sleep(programs: &Programs) -> Result<Child, String> {
    child_command(&programs.caffeinate)
        .args(["-is", "-w", &std::process::id().to_string()])
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "sleep_prevention".into())
}

struct ResolvedBackend {
    host: String,
    user: String,
    port: u16,
    host_key_alias: Option<String>,
    known_hosts: Option<String>,
}

impl ResolvedBackend {
    fn parse(config: &str) -> Result<Self, String> {
        let get = |key: &str| {
            config.lines().find_map(|line| {
                let (name, value) = line.split_once(' ')?;
                (name == key).then(|| value.trim().to_owned())
            })
        };
        Ok(Self {
            host: get("hostname")
                .filter(|s| !s.is_empty())
                .ok_or("settings")?,
            user: get("user").filter(|s| !s.is_empty()).ok_or("settings")?,
            port: get("port")
                .and_then(|s| s.parse().ok())
                .filter(|p| *p > 0)
                .ok_or("settings")?,
            host_key_alias: get("hostkeyalias").filter(|s| s != "none"),
            known_hosts: get("userknownhostsfile"),
        })
    }
}

fn master_arguments(
    settings: &TunnelSettings,
    socket: &Path,
    backend: &ResolvedBackend,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-F",
        "/dev/null",
        "-N",
        "-T",
        "-M",
        "-o",
        "ControlPersist=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "PasswordAuthentication=no",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "RemoteCommand=none",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.extend([
        "-S".into(),
        socket.to_string_lossy().into(),
        "-i".into(),
        settings.identity_file.clone(),
    ]);
    args.extend([
        "-p".into(),
        settings.ssh_port.unwrap_or(backend.port).to_string(),
        "-l".into(),
        backend.user.clone(),
    ]);
    if let Some(alias) = &backend.host_key_alias {
        args.extend(["-o".into(), format!("HostKeyAlias={alias}")]);
    }
    if let Some(paths) = &backend.known_hosts {
        args.extend(["-o".into(), format!("UserKnownHostsFile={paths}")]);
    }
    #[cfg(target_os = "macos")]
    args.extend(["-o".into(), "UseKeychain=yes".into()]);
    args.extend(["--".into(), backend.host.clone()]);
    args
}

fn child_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true);
    command
}

impl SessionProcesses {
    async fn connect(
        settings: &TunnelSettings,
        port: u16,
        programs: &Programs,
    ) -> Result<Self, String> {
        // Resolve only connection coordinates and host trust from the alias.
        // Start the tunnel without its config so extra IdentityFile entries,
        // forwarding directives and LocalCommand cannot bypass the chosen key.
        let resolved = timeout(
            Duration::from_secs(3),
            Command::new(&programs.ssh)
                .args(["-G", "--", &settings.backend_host])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "settings")?
        .map_err(|_| "ssh_start")?;
        if !resolved.status.success() {
            return Err("settings".into());
        }
        let backend = ResolvedBackend::parse(&String::from_utf8_lossy(&resolved.stdout))?;
        let directory = ControlDirectory::create()?;
        let socket = directory.socket();
        let mut ssh = owned_ssh(&programs.ssh, master_arguments(settings, &socket, &backend))?;
        // Control checks do not fall back to a new SSH connection. A successful
        // `-O forward` acknowledges the server's actual bind, unlike checking
        // only whether an ssh process exists.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if ssh.try_wait().map_err(|_| "ssh_start")?.is_some() {
                return Err(read_ssh_error(&mut ssh).await);
            }
            let mut check = child_command(&programs.ssh);
            check
                .args([
                    "-F",
                    "/dev/null",
                    "-S",
                    socket.to_str().ok_or("ssh_start")?,
                    "-O",
                    "check",
                    "--",
                    &backend.host,
                ])
                .stderr(Stdio::null());
            if matches!(timeout(Duration::from_secs(1), check.status()).await, Ok(Ok(status)) if status.success())
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err("ssh_connect".into());
            }
            sleep(Duration::from_millis(100)).await;
        }
        let forward = format!("localhost:{port}:localhost:{}", programs.local_port);
        let output = timeout(
            Duration::from_secs(10),
            child_command(&programs.ssh)
                .args([
                    "-F",
                    "/dev/null",
                    "-S",
                    socket.to_str().ok_or("ssh_start")?,
                    "-O",
                    "forward",
                    "-o",
                    "ExitOnForwardFailure=yes",
                    "-R",
                    &forward,
                    "--",
                    &backend.host,
                ])
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| "forward")?
        .map_err(|_| "forward")?;
        if !output.status.success() {
            return Err(classify_ssh_error(&String::from_utf8_lossy(&output.stderr)));
        }
        Ok(Self {
            _lease: ssh.stdin.take(),
            ssh,
            _directory: directory,
        })
    }

    async fn error(&mut self) -> String {
        read_ssh_error(&mut self.ssh).await
    }
}

async fn read_ssh_error(ssh: &mut Child) -> String {
    let mut bytes = Vec::new();
    if let Some(stderr) = ssh.stderr.take() {
        let _ = timeout(
            Duration::from_secs(1),
            stderr.take(8192).read_to_end(&mut bytes),
        )
        .await;
    }
    classify_ssh_error(&String::from_utf8_lossy(&bytes))
}

fn classify_ssh_error(stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("host key verification failed")
        || lower.contains("host identification has changed")
    {
        "host_key"
    } else if lower.contains("permission denied") || lower.contains("sign_and_send_pubkey") {
        "ssh_auth"
    } else if lower.contains("forward") || lower.contains("port forwarding") {
        "forward"
    } else {
        "ssh_connect"
    }
    .into()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Mutex as StdMutex};
    use tokio::{io::AsyncWriteExt, net::TcpListener};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ft-test-{}", Uuid::new_v4().simple()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
        fn settings(&self) -> TunnelSettings {
            TunnelSettings {
                backend_host: "backend".into(),
                ssh_port: None,
                identity_file: self
                    .file("key", "test private key")
                    .to_string_lossy()
                    .into(),
            }
        }
        fn programs(&self, port: u16, forward_fails: bool) -> Programs {
            // Only scripts in this isolated fixture are executed. No real SSH
            // account, keychain credential or remote host is used by these tests.
            let ssh = self.file(
                "ssh",
                &format!(
                    r#"#!/bin/sh
if [ "$1" = '-G' ]; then
  printf 'hostname backend.test\nuser tunnel\nport 22\n'
  exit 0
fi
for arg in "$@"; do
  if [ "$arg" = 'check' ]; then exit 0; fi
  if [ "$arg" = 'forward' ]; then
    if [ '{forward_fails}' = 'true' ]; then
      echo 'remote port forwarding failed' >&2
      exit 255
    fi
    exit 0
  fi
done
echo $$ > '{}/ssh.pid'
exec /bin/sleep 60
"#,
                    self.0.display()
                ),
            );
            let caffeinate = self.file(
                "caffeinate",
                &format!(
                    "#!/bin/sh\necho $$ > '{}/awake.pid'\nexec /bin/sleep 60\n",
                    self.0.display()
                ),
            );
            Programs {
                ssh,
                caffeinate,
                local_port: port,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Clone)]
    struct TestReporter {
        calls: Arc<StdMutex<Vec<bool>>>,
        current: bool,
        on_error: Option<String>,
    }
    impl TestReporter {
        fn new() -> Self {
            Self {
                calls: Arc::default(),
                current: true,
                on_error: None,
            }
        }
    }
    impl Reporter for TestReporter {
        fn current(&self) -> bool {
            self.current
        }
        async fn report(&self, on: bool) -> Result<TunnelRegistration, String> {
            self.calls.lock().unwrap().push(on);
            if on {
                if let Some(error) = &self.on_error {
                    return Err(error.clone());
                }
            }
            Ok(TunnelRegistration {
                device_id: "test-device".into(),
                port: 30001,
            })
        }
        async fn register_key(&self, _identity_file: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn manager(settings: TunnelSettings) -> TunnelManager {
        let (desired, _) = watch::channel(None);
        let (status, _) = watch::channel(TunnelStatus {
            supported: true,
            enabled: true,
            phase: "connecting".into(),
            settings,
            port: None,
            error: None,
            sync_error: None,
            retry_in_seconds: None,
        });
        TunnelManager {
            inner: Arc::new(Inner {
                path: PathBuf::new(),
                app: None,
                desired,
                status,
                operation: Arc::new(Mutex::new(())),
                quitting: AtomicBool::new(false),
                exit_ready: AtomicBool::new(false),
            }),
        }
    }

    async fn ssh_banner() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"SSH-2.0-test\r\n").await;
            }
        });
        (port, task)
    }

    async fn wait_phase(manager: &TunnelManager, phase: &str) {
        let mut status = manager.inner.status.subscribe();
        timeout(Duration::from_secs(4), async {
            loop {
                if status.borrow_and_update().phase == phase {
                    return;
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("worker did not reach expected phase");
    }

    async fn assert_reaped(path: PathBuf) {
        if !path.exists() {
            return;
        }
        let pid = fs::read_to_string(path).unwrap();
        timeout(Duration::from_secs(3), async {
            loop {
                if !Command::new("/bin/kill")
                    .args(["-0", pid.trim()])
                    .stderr(Stdio::null())
                    .status()
                    .await
                    .unwrap()
                    .success()
                {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("owned child process survived cancellation");
    }

    #[test]
    fn settings_reject_options_and_shared_private_keys() {
        let fixture = Fixture::new();
        let settings = fixture.settings();
        for host in [
            "",
            "-oProxyCommand=bad",
            "backend;id",
            "host name",
            "user\nhost",
        ] {
            let mut invalid = settings.clone();
            invalid.backend_host = host.into();
            assert_eq!(invalid.validated().unwrap_err(), "settings");
        }
        let mut invalid = settings.clone();
        invalid.ssh_port = Some(0);
        assert_eq!(invalid.validated().unwrap_err(), "settings");
        fs::set_permissions(&settings.identity_file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(settings.validated().unwrap_err(), "key_permissions");
    }

    #[test]
    fn master_uses_only_selected_key_and_preserves_host_trust() {
        let backend = ResolvedBackend::parse("hostname example.test\nuser user\nport 2222\nhostkeyalias trusted-backend\nuserknownhostsfile /tmp/known_hosts\n").unwrap();
        let fixture = Fixture::new();
        let settings = fixture.settings();
        let args = master_arguments(&settings, Path::new("/tmp/control"), &backend);
        assert_eq!(&args[..2], ["-F", "/dev/null"]);
        assert!(args
            .windows(2)
            .any(|p| p == ["-i", &settings.identity_file]));
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 1);
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"HostKeyAlias=trusted-backend".into()));
        assert!(args.contains(&"ClearAllForwardings=yes".into()));
        assert!(args.windows(2).any(|p| p == ["-p", "2222"]));
    }

    #[test]
    fn settings_round_trip_is_private_and_does_not_store_enablement() {
        let fixture = Fixture::new();
        let settings = fixture.settings();
        let path = fixture.0.join("settings.json");
        save_settings(&path, &settings).unwrap();
        let data: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(data["backendHost"], "backend");
        assert!(data.get("enabled").is_none());
        assert!(data.get("accessToken").is_none());
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(50), Duration::from_secs(60));
    }

    #[test]
    fn forwarding_failure_never_reports_on_and_cleans_up_ssh() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let manager = manager(settings.clone());
            let (port, banner) = ssh_banner().await;
            let reporter = TestReporter::new();
            let result = run_connection(
                &manager,
                &settings,
                &reporter,
                &fixture.programs(port, true),
            )
            .await;
            assert_eq!(result.unwrap_err(), "forward");
            assert_eq!(*reporter.calls.lock().unwrap(), [false]);
            assert_ne!(manager.status().phase, "connected");
            assert_reaped(fixture.0.join("ssh.pid")).await;
            banner.abort();
        });
    }

    #[test]
    fn connected_tunnel_cancellation_reaps_ssh_and_sleep_prevention() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let manager = manager(settings.clone());
            let (port, banner) = ssh_banner().await;
            let programs = fixture.programs(port, false);
            let reporter = TestReporter::new();
            let worker_manager = manager.clone();
            let worker_reporter = reporter.clone();
            let task = tokio::spawn(async move {
                run_enabled(&worker_manager, &settings, &worker_reporter, &programs).await
            });
            wait_phase(&manager, "connected").await;
            assert_eq!(*reporter.calls.lock().unwrap(), [false, true]);
            assert_eq!(manager.status().port, Some(30001));
            // An open lifetime pipe must keep SSH alive, even when idle. This
            // catches shell read-timeout behavior on macOS as well as EOF bugs.
            sleep(Duration::from_millis(1200)).await;
            assert!(
                !task.is_finished(),
                "an idle tunnel exited while its app pipe was open"
            );
            task.abort();
            let _ = task.await;
            assert_reaped(fixture.0.join("ssh.pid")).await;
            assert_reaped(fixture.0.join("awake.pid")).await;
            banner.abort();
        });
    }

    #[test]
    fn failed_registration_reports_off_before_retrying() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let manager = manager(settings.clone());
            let (port, banner) = ssh_banner().await;
            let programs = fixture.programs(port, false);
            let mut reporter = TestReporter::new();
            reporter.on_error = Some("network".into());
            let worker_manager = manager.clone();
            let worker_reporter = reporter.clone();
            let task = tokio::spawn(async move {
                run_enabled(&worker_manager, &settings, &worker_reporter, &programs).await
            });
            wait_phase(&manager, "reconnecting").await;
            assert_eq!(*reporter.calls.lock().unwrap(), [false, true, false]);
            assert_eq!(manager.status().retry_in_seconds, Some(1));
            assert_eq!(manager.status().error.as_deref(), Some("network"));
            task.abort();
            let _ = task.await;
            assert_reaped(fixture.0.join("ssh.pid")).await;
            assert_reaped(fixture.0.join("awake.pid")).await;
            banner.abort();
        });
    }

    #[test]
    fn changed_account_cannot_start_a_tunnel() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let manager = manager(settings.clone());
            let mut reporter = TestReporter::new();
            reporter.current = false;
            let result = run_enabled(&manager, &settings, &reporter, &Programs::default()).await;
            assert_eq!(result.unwrap_err(), "auth");
            assert!(reporter.calls.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn preflight_requires_an_ssh_service() {
        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                stream.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();
            });
            assert_eq!(ensure_sshd(port).await.unwrap_err(), "sshd");
            server.await.unwrap();
        });
    }

    #[test]
    fn immediate_off_is_acknowledged_before_worker_observes_on() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let manager = manager(settings.clone());
            let receiver = manager.inner.desired.subscribe();
            manager.inner.desired.send_replace(Some(Request {
                settings,
                identity: TunnelIdentity {
                    user_id: "test".into(),
                    device_id: "device".into(),
                },
            }));
            let stopping = manager.clone();
            let stop = tokio::spawn(async move {
                stopping.stop().await;
            });
            wait_phase(&manager, "stopping").await;
            let worker_manager = manager.clone();
            let worker = tokio::spawn(async move {
                worker_manager.supervise(receiver).await;
            });
            timeout(Duration::from_secs(1), stop)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(manager.status().phase, "off");
            worker.abort();
            let _ = worker.await;
        });
    }

    #[test]
    fn account_transition_blocks_enable_until_credentials_are_changed() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let manager = manager(fixture.settings());
            let transition = manager.stop_for_account_change().await;
            assert!(
                timeout(Duration::from_millis(30), manager.inner.operation.lock())
                    .await
                    .is_err()
            );
            drop(transition);
            assert!(
                timeout(Duration::from_secs(1), manager.inner.operation.lock())
                    .await
                    .is_ok()
            );
        });
    }

    #[test]
    fn lost_app_pipe_terminates_and_reaps_the_ssh_child() {
        tauri::async_runtime::block_on(async {
            let fixture = Fixture::new();
            let settings = fixture.settings();
            let programs = fixture.programs(22, false);
            let mut processes = SessionProcesses::connect(&settings, 30001, &programs)
                .await
                .unwrap();
            // Closing the parent end is also what happens on app SIGKILL.
            drop(processes._lease.take());
            timeout(Duration::from_secs(3), processes.ssh.wait())
                .await
                .unwrap()
                .unwrap();
            assert_reaped(fixture.0.join("ssh.pid")).await;
        });
    }

    #[test]
    #[ignore = "opens a temporary localhost-only forward on an explicitly configured SSH server"]
    fn live_reverse_forward_round_trip_and_cleanup() {
        tauri::async_runtime::block_on(async {
            let host =
                std::env::var("FASTADE_TEST_TUNNEL_HOST").expect("set FASTADE_TEST_TUNNEL_HOST");
            let key =
                std::env::var("FASTADE_TEST_TUNNEL_KEY").expect("set FASTADE_TEST_TUNNEL_KEY");
            let port: u16 = std::env::var("FASTADE_TEST_TUNNEL_PORT")
                .expect("set FASTADE_TEST_TUNNEL_PORT")
                .parse()
                .unwrap();
            let settings = TunnelSettings {
                backend_host: host.clone(),
                ssh_port: None,
                identity_file: key,
            }
            .validated()
            .unwrap();
            let (local_port, banner) = ssh_banner().await;
            let programs = Programs {
                local_port,
                ..Programs::default()
            };
            let fixture = Fixture::new();
            for _ in 0..2 {
                let awake = prevent_sleep(&programs).unwrap();
                let processes = SessionProcesses::connect(&settings, port, &programs)
                    .await
                    .unwrap();
                fixture.file("ssh.pid", &processes.ssh.id().unwrap().to_string());
                fixture.file("awake.pid", &awake.id().unwrap().to_string());
                let script = format!("python3 -c 'import socket; s=socket.create_connection((\"localhost\",{port}),5); print(s.recv(256).decode().strip()); s.close()'");
                let output = timeout(
                    Duration::from_secs(15),
                    Command::new("/usr/bin/ssh")
                        .args(["-T", "-o", "BatchMode=yes", "--", &host, &script])
                        .stdin(Stdio::null())
                        .kill_on_drop(true)
                        .output(),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(
                    output.status.success(),
                    "remote probe failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    String::from_utf8_lossy(&output.stdout).trim(),
                    "SSH-2.0-test"
                );
                let listeners = Command::new("/usr/bin/ssh")
                    .args([
                        "-T",
                        "-o",
                        "BatchMode=yes",
                        "--",
                        &host,
                        &format!("ss -H -ltn 'sport = :{port}'"),
                    ])
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output()
                    .await
                    .unwrap();
                assert!(listeners.status.success());
                let text = String::from_utf8_lossy(&listeners.stdout);
                assert!(!text.trim().is_empty());
                assert!(
                    text.lines().all(|line| {
                        let address = line.split_whitespace().nth(3).unwrap_or("");
                        address.starts_with("127.0.0.1:") || address.starts_with("[::1]:")
                    }),
                    "forward was not loopback-only"
                );
                drop(processes);
                drop(awake);
                assert_reaped(fixture.0.join("ssh.pid")).await;
                assert_reaped(fixture.0.join("awake.pid")).await;
                // The SSH server needs a brief moment to observe disconnect.
                sleep(Duration::from_millis(300)).await;
            }
            banner.abort();
        });
    }
}
