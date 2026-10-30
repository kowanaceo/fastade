# Desktop reverse SSH tunnel

Settings → Desktop tunnel controls one Mac → backend connection independently
of terminal sessions. It forwards `backend localhost:<assigned port>` to the
Mac's `localhost:22`. It does not attach a phone to an existing terminal
session. Phone login through the backend is described below.

## Setup

- Sign in to fastade. The backend identifies the device from its access token.
- Enable Remote Login in macOS sharing settings. Configure sshd for key-only
  authentication (`PasswordAuthentication no`, `KbdInteractiveAuthentication no`)
  and register the phone's public key for the intended Mac account. fastade
  checks for a local SSH service but does not change system login permissions.
- SSH key: nothing to prepare. When the toggle is first turned on (or the
  settings saved) and `~/.ssh/fastade-tunnel` does not exist, fastade creates it
  with `ssh-keygen -t ed25519` (no passphrase, mode `600`). Each connection
  uploads only the public key (`ssh-ed25519 <base64>`, derived with
  `ssh-keygen -y`) to `PUT /tunnels/me/key`; the backend writes the restricted
  `authorized_keys` entry for the device's assigned port. Signing out clears the
  key on the backend, and the next connection uploads it again.
  An existing file at the configured path is never overwritten, so you may
  select your own unencrypted ed25519 key instead. Private key contents are
  never copied into app settings, sync or the WebView.
- Enter the backend SSH host or alias and an optional SSH port, then save.
- Verify the backend host fingerprint through your normal SSH connection so
  its host key is already trusted in `known_hosts`. The tunnel never accepts
  an unknown or changed host key automatically.
- Turn the toggle on. Errors `key_in_use` (the key belongs to another device)
  and `key_rejected` stop the tunnel with a message; choose a different key file.

The backend must use `GatewayPorts no`. Its tunnel-state task documents this
restricted `authorized_keys` entry (replace the port and public key):

```text
restrict,port-forwarding,permitlisten="localhost:30000",command="/usr/sbin/nologin" ssh-ed25519 <public-key> fastade-tunnel-<deviceId>
```

The app uses key authentication without an interactive password prompt.
Passphrase-protected keys are not supported because the public key is derived
non-interactively; use the auto-generated key or an unencrypted ed25519 key.
SSH aliases contribute the resolved hostname, username, port, host-key alias
and user known-hosts paths. Other alias directives, including ProxyJump,
ProxyCommand, extra identities and existing forwards, are not inherited by the
tunnel: it connects directly to the backend using the selected key.

## Phone login through the backend

The backend binds the forwarded port to loopback (`GatewayPorts no`), so a phone
cannot reach it directly. Settings → Phone login (aissh) creates one ed25519
key per QR and the phone signs in with it twice:

```text
phone ──SSH :22, key──▶ backend jump account ──forward localhost:<port>──▶ tunnel ──▶ Mac sshd (same key)
```

- With the tunnel connected the app uploads the public key to the backend,
  appends it to the Mac's `~/.ssh/authorized_keys`, and shows a one-time QR
  (60 seconds). The private key exists only in the QR.
- With the tunnel off the QR falls back to this Mac's Wi-Fi address (same
  network only) and no jump host is included.
- Revoke removes the key from `authorized_keys` and from the backend. Either
  removal alone already blocks the phone.

QR link parameters (`aissh://add-server`): `host`, `port`, `user`, `name`,
`key` (private key, base64url) as before. When there is a jump host the QR adds
`jump_host`, `jump_port`, `jump_user`; `host`/`port` are then the target as the
backend sees it (`localhost` and the device's tunnel port), and the phone must
request exactly that host name when opening the forward.

Backend API (same bearer auth as `/tunnels/me`):

```text
PUT    /tunnels/me/phone-keys  {publicKey: "ssh-ed25519 <base64>", label}
       -> {jumpHost, jumpPort, jumpUser, targetHost}
DELETE /tunnels/me/phone-keys  {publicKey}   (unknown key -> 404, treated as done)
```

The backend writes the key to the jump account's `authorized_keys` as
`restrict,port-forwarding,permitopen="<targetHost>:<device port>",command="/usr/sbin/nologin" <key>`.
Errors: 409 key owned by another account, 422 invalid key.

## Runtime and API contract

```text
PUT /tunnels/me  {on: false, hostname} -> {deviceId, port, ...}
  resolve the device's stable, server-assigned port
  authenticate SSH; receive remote-forward acknowledgement (local sshd is checked first)
PUT /tunnels/me  {on: true, hostname}
  repeat every 30 seconds while connected
PUT /tunnels/me  {on: false, hostname}
  on disconnect, OFF, logout/account change, or normal app exit
```

The initial on=false report never advertises an unverified tunnel. OpenSSH is
an app-owned foreground master; a separate `ssh -O forward` request waits for
the backend's forwarding acknowledgement. A live process alone does not mean
the port was opened. `BatchMode`, `ExitOnForwardFailure`, strict host-key
checking and server keepalives are enabled. A private temporary control socket
prevents sharing another session's connection. A static lifetime guard owns SSH
and receives a pipe from the app. EOF (also on app force-quit) kills and reaps
SSH, preventing an orphaned listener from holding the assigned port. All user
values are literal process arguments; no shell evaluates them as code.

Transient failures reconnect after 1, 2, 4, 8, 16, 32 and at most 60 seconds;
authentication/host-key/setup errors stop with an actionable message. OFF
cancels an in-flight connection or retry. Settings can change only while OFF.
Multiple windows share the same supervisor and receive `tunnel-changed` events.

While enabled (including reconnect/backoff), `caffeinate -is -w <app-pid>`
prevents idle sleep and system sleep on AC power. The display may sleep. Lid
closure, explicit Sleep and OS termination can still interrupt the connection.
SSH is killed and reaped when a connection is cancelled; caffeinate stays alive
across reconnects and is killed when the enabled run ends. Normal app exit waits for
the bounded off report before exiting, including repeated quit requests.

The backend currently declares a 90-second stale threshold. The 30-second
heartbeat requires that threshold to remain longer than 30 seconds. If an off
report cannot be delivered, stale expiry makes the backend report off. A
heartbeat failure shows a warning rather than claiming successful sync.
The current account/device is checked before each report; enable is serialized
with the entire logout/account-switch operation. Backend tokens stay in Rust
and the OS keychain.

Only host, SSH port and private-key path are saved locally in
`tunnel-settings.json` (mode `600` on Unix, replaced atomically). Tunnel settings
are not account-sync settings. The enabled flag is not persisted: restarting
the app starts with the tunnel off. macOS is the supported execution platform;
other desktop platforms show an unsupported message.

## Verification

Automated tests cover input/key permission validation, connection arguments,
settings persistence, SSH service preflight, failed-forward reporting,
reconnection backoff, cancellation/process cleanup, immediate ON→OFF, and
account-transition serialization. Run:

```sh
npm run check
npm run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
```

The opt-in `live_reverse_forward_round_trip_and_cleanup` test uses the actual
OpenSSH and caffeinate processes with an isolated local SSH-banner listener.
Set `FASTADE_TEST_TUNNEL_HOST`, `FASTADE_TEST_TUNNEL_KEY`, and an available
`FASTADE_TEST_TUNNEL_PORT`, then run it with `--ignored`. It opens a temporary
remote loopback listener twice, checks the banner from the backend, verifies
loopback-only binding, and verifies both local child processes are reaped.
On 2026-10-29 KST this passed against backend. No sshd configuration or
authorized_keys were modified by the test, and the production Mac sshd was not
used.
