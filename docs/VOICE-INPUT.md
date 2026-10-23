# Voice input

The desktop client implements the session task “음성 입력(STT): 로그인 시에만 음성→텍스트를 AI CLI 세션에 전달”. Google sign-in already exchanges identity for an aisshapi backend session; voice input uses that same account, without another login or copying tokens into the WebView or SSH server.

## Flow

- A connected session has a microphone button, disabled while signed out.
- Opening the dialog pins its destination to that session ID and starts recording after microphone permission is granted. The last selected speech language is remembered.
- MediaRecorder records MP4/M4A when supported, otherwise WebM/Opus. Recording stops at 60 seconds, with a 10 MiB upload cap.
- Stop uploads audio and automatically submits the recognized text to the pinned session. If submission fails, the dialog keeps editable text for review and manual retry. Cancellation, logout, and dialog destruction release the microphone, discard audio, and abort an active client HTTP request. A server may already have begun processing an uploaded request.
- Network, timeout, authentication, unsupported format, quota, empty recognition, and service availability failures have Korean/English messages. Transcription retries reuse the in-memory recording; no automatic retries occur except one credential refresh on HTTP 401.
- The destination remains fixed when selection changes. Disconnection disables sending; a failed send preserves the edited text. A PTY write error may be partial, so users are asked to check their terminal before retrying.
- Input control characters are removed and line breaks become spaces, followed by one Enter. The same session-write queue and activity tracking handle local and SSH sessions. The dialog's Escape closes it without interrupting the underlying agent.
- New UI strings use Korean/English resources, selected from the system language. Speech language is selected separately: automatic, `ko`, or `en`.

## API contract and deployment dependency

The desktop follows the contract recorded in the aisshapi session task `8ae94cc3-6676-4997-ad5e-7f1ccca10890`:

```http
POST https://backend.kowanas.com/aisshapi/stt/transcribe
Authorization: Bearer <backend access token>
Content-Type: multipart/form-data; boundary=...

file: voice.m4a (audio/mp4) or voice.webm (audio/webm)
language: ko | en (omitted for automatic detection)
```

```json
{"text":"인식된 텍스트", "language":"ko", "durationSec":3.2}
```

Only `text` is required by this client. Credentials remain in Rust/OS Keychain. A 401 refreshes credentials through the existing serialized refresh flow, persists the rotated tokens, and retries the upload once. Connect timeout is 10 seconds and each HTTP request has a 90-second deadline. Redirects are disabled. Client errors never include upstream response bodies, audio, or transcript text.

At implementation time (2026-10-19 KST), the backend session task remained `todo`, and the inspected `/home/ubuntu/workspace/aisshapi/app` contained no STT implementation. The public `/openapi.json` route returned Not Found. The client reports an availability error on HTTP 404/405/501. Server implementation/deployment and a real signed-in microphone-to-CLI check remain prerequisites for end-to-end acceptance; client tests do not establish those outcomes.

## macOS

`Info.plist` declares the microphone usage reason, and `Entitlements.plist` enables audio input for signed hardened-runtime builds. These follow [Tauri's macOS bundle instructions](https://tauri.app/distribute/macos-application-bundle/) and [Apple's audio input entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input). No speech recognition entitlement is needed because transcription runs on the backend.

## Verification

```sh
npm run check
npm run test:voice
npm run test:activity
npm run test:usage
npm run build
cd apps/desktop/src-tauri && cargo test
```

Automated voice tests cover automatic submission, review and retry after a send failure, Korean/English text, late microphone permission after cancellation, cancellation during recognition, retrying retained audio, duplicate send suppression, permission denial, empty/oversize uploads, and terminal control characters. Rust tests cover browser MIME formats, optional language, and upload validation.

Verified during implementation: Svelte check passed with no warnings, all 44 JavaScript tests and 62 Rust tests passed (4 existing environment-dependent Rust tests ignored), the Vite production build passed, and `npm run tauri -- build --debug --bundles app` produced a macOS app. The built app's Info.plist contains the microphone usage reason. The debug executable has a linker ad-hoc signature; distribution signing and entitlement application still require the release build/signing process. Full-repository `cargo fmt --check` also reports existing formatting differences in unrelated Rust files; formatting checks on the modified Rust implementation files pass.

After server deployment, verify a bundled desktop build with microphone permission allowed/denied, Korean/English/automatic recognition, offline/timeout/quota errors, cancellation during recording/upload, logout, local/SSH destinations, session disconnection, and application closure while recording. The `NSMicrophoneUsageDescription` key and audio entitlement must be present in the distributed app. A standalone browser cannot verify Tauri IPC or macOS permissions.
