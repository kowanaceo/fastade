# fastade 제품·기술 명세서

- 문서 상태: Draft v0.1
- 작성일: 2026-09-15
- 대상 플랫폼: macOS, Windows, Linux, iOS, Android
- 데스크톱: Tauri 2 + Rust
- 모바일: Flutter

## 1. 제품 정의

fastade는 Codex CLI, Gemini CLI, Claude Code를 한 인터페이스에서 실행하고, 세션·권한·프로젝트·사용량을 통합 관리하는 멀티플랫폼 클라이언트다.

데스크톱 앱은 사용자 컴퓨터에서 실제 CLI 프로세스를 실행하는 **호스트**다. 모바일 앱은 데스크톱 호스트에 안전하게 연결하여 세션을 조회·제어하는 **컴패니언**이다. 모바일 OS 제약상 iOS/Android에서 임의의 AI CLI 바이너리를 직접 실행하는 기능은 MVP 범위에 포함하지 않는다.

### 1.1 목표

- 하나의 앱에서 여러 AI CLI를 등록, 실행, 전환한다.
- 각 AI CLI의 원래 대화형 기능과 화면을 PTY 터미널로 손실 없이 제공한다.
- 여러 프로젝트와 여러 동시 세션을 안전하게 관리한다.
- 모바일에서 진행 상황 확인, 메시지 전송, 권한 승인/거부, 중단을 수행한다.
- CLI가 추가되어도 앱 본체 변경을 최소화하는 어댑터 구조를 제공한다.
- 로컬 우선(local-first)으로 동작하고 사용자의 코드와 자격 증명을 외부 중계 서버에 저장하지 않는다.

## 2. 대상 사용자와 핵심 시나리오

### 2.1 대상 사용자

- 둘 이상의 AI 코딩 CLI를 프로젝트별로 병행하는 개발자
- 장시간 실행되는 에이전트 작업을 자리 밖에서 확인하려는 사용자
- CLI별 승인 요청과 결과물을 한곳에서 관리하려는 팀 또는 개인

### 2.2 핵심 사용자 흐름

1. 사용자가 server(local 또는 SSH 원격)를 고르고 ▶를 누르면 그 server의 `~`에서 login shell 세션이 열린다.
2. 세션 헤더의 폴더 버튼으로 작업 folder를 고른다(local은 OS 폴더 선택창, 원격은 폴더 탐색기). shell이 그 folder로 이동하고 세션 이름이 폴더 이름이 된다. pin하면 이 server+folder가 저장된다.
3. 세션 헤더의 AI 선택으로 Codex/Claude Code/Gemini를 그 shell 안에서 실행한다(설정의 CLI별 기본 model 적용). 앱은 PTY 원본 스트림을 그대로 보여주고, 구조화 출력은 공통 이벤트로 변환한다.
4. 사용자는 CLI 원래 터미널에서 대화, slash command, 승인 요청과 도구 실행을 그대로 사용한다. ■ 를 누르면 agent가 종료되고 shell로 돌아온다.
5. 앱을 닫았다 다시 열어도 세션 이력과 메타데이터가 복원된다.
6. pin된 프로젝트를 누르면 저장된 server의 저장된 folder에서 바로 shell이 열린다.
7. 모바일에서 원격 server(SSH)를 등록한 뒤 원격 상태 확인, 입력, 승인/거부, 중단을 수행한다. 모바일은 로컬 실행이 없으므로 server 목록 중 SSH 대상만 선택할 수 있다.

## 3. MVP 범위

- macOS 데스크톱 우선, 이후 Windows/Linux 빌드 검증
- CLI 자동 탐지 및 수동 실행 경로 설정
- 공식 어댑터 3종: Codex CLI, Claude Code, Gemini CLI
- 일반 터미널 출력용 Generic CLI 어댑터
- 프로젝트 등록 및 최근 프로젝트 목록
- 세션 생성, 입력, 스트리밍 출력, 중단, 재연결, 이력 조회
- CLI별 PTY 원본 스트림과 터미널 scrollback 보존
- 승인 요청 표시 및 승인/거부
- 파일 변경 요약과 diff 열람
- 모바일 SSH 호스트 등록, 세션 목록/상세, 메시지 전송, 승인/거부, 중단
- 로컬 SQLite 저장소, OS 보안 저장소 기반 비밀 관리
- 앱/CLI 오류 진단 번들 생성(비밀 자동 마스킹)
- 병렬 세션, 세션 복제 및 CLI 간 컨텍스트 전달
- SSH 원격 호스트
- 푸시 알림
- 음성 입력, 이미지/파일 첨부
- 플러그인 SDK 및 커뮤니티 어댑터
- 비용 예산과 사용량 대시보드
- Git worktree 자동 생성 및 브랜치 관리

## 4. 시스템 아키텍처

```text
Desktop UI (Tauri WebView)             Mobile UI (Flutter)
          | Tauri IPC                         | SSH channel
          v                                   v
+------------------+                   +------------------+
| Desktop Client   |                   | Dart Client      |
+--------+---------+                   +--------+---------+
         | local IPC                            | stdio
         v                                      v
+---------------------------+       +----------------------+
| Rust Host/Daemon          |<------| SSH stdio gateway   |
| Core | Runtime | Storage  |       +----------------------+
+-------------+-------------+
              | PTY / pipes / subprocess
              v
  Codex | Claude | Gemini | Generic CLI
```

Rust host/daemon이 AI 세션, CLI 자식 프로세스, SQLite를 단독 소유한다. Tauri 앱은 로컬 IPC로 host에 연결하고, 모바일의 `fastade host --stdio` 명령은 SSH stdio와 로컬 host IPC 사이를 중계한다. 따라서 데스크톱 창이나 모바일 SSH 연결이 종료되어도 실행 중인 AI 세션은 유지된다.

### 4.1 설계 원칙

- **로컬 우선:** 프로젝트 파일, 전체 프롬프트, CLI 자격 증명은 기본적으로 호스트 밖으로 보내지 않는다.
- **CLI-native 우선:** 기본 화면은 PTY 원본 스트림을 xterm으로 렌더링하며 CLI의 기능과 키 입력을 임의로 축소하지 않는다.
- **선택적 정규화:** 구조화 이벤트는 알림·상태 요약 같은 보조 기능에만 사용하고 원본 터미널을 대체하지 않는다.
- **권한 최소화:** 프로젝트별 허용 경로와 명령 정책을 적용한다.
- **기능 협상:** CLI/호스트/모바일의 버전 차이를 capability negotiation으로 처리한다.
- **점진적 저하:** 구조화 출력이 없는 CLI도 텍스트 스트림으로 사용할 수 있어야 한다.

### 4.2 권장 모노레포 구조

```text
apps/
  desktop/                 # Tauri 2 앱 + 웹 UI
  mobile/                  # Flutter iOS/Android 앱
crates/
  core/                    # 세션, 정책, 도메인 로직
  host/                    # 로컬 daemon과 lifecycle
  protocol/                # 공통 메시지 및 버전 관리
  storage/                 # SQLite/repository 계층
  cli-runtime/             # 프로세스, PTY, 종료/복구
  adapters/
    codex/
    claude/
    gemini/
    generic/
packages/
  protocol-schema/         # JSON Schema 또는 protobuf 원본
  desktop-ui/              # Svelte 5 + TypeScript + Vite UI 패키지
  dart-sdk/                # 스키마에서 생성된 Dart 모델/클라이언트
docs/
  adr/                     # Architecture Decision Records
  protocol/
```

Tauri는 데스크톱 앱의 WebView, 네이티브 패키징, Rust IPC를 제공하는 애플리케이션 프레임워크다. WebView 내부 화면은 Svelte 5 + TypeScript + Vite로 구성한다. 웹 서버와 SSR이 필요하지 않으므로 SvelteKit은 사용하지 않는다. Svelte 의존성은 View와 ViewModel에만 두고 Tauri IPC 경계 뒤의 Rust 코어에는 두지 않는다.

### 4.3 ViewModel 분리

Desktop과 Mobile 모두 `View → ViewModel → Application Client → Host Domain/Core` 방향으로만 의존한다.

```text
Desktop: Svelte View -> TypeScript ViewModel -> Tauri/Local IPC Client -> Rust Host
Mobile:  Flutter View -> Dart ViewModel      -> SSH/Stdio Client      -> Rust Host
```

- View는 렌더링, 사용자 제스처 전달, 플랫폼 UI만 담당한다.
- ViewModel은 화면 상태, 로딩/오류 상태, command 호출, event 구독을 담당한다.
- ViewModel은 파일 시스템, SQLite, SSH, Tauri API 또는 CLI 프로세스를 직접 호출하지 않는다.
- Rust core의 `Session`, `Project`, `Approval`과 화면용 ViewModel/DTO를 분리한다.
- 프로토콜 DTO를 View에 그대로 노출하지 않고 ViewModel에서 표시 모델로 변환한다.
- Desktop ViewModel과 Mobile ViewModel은 구현을 공유하지 않되 동일한 상태 전이 fixture로 동작을 검증한다.
- 세션 상세, 프로젝트 목록, 승인 요청 등 기능 단위 ViewModel을 만들고 전역 mutable state는 두지 않는다.
- 일회성 UI 효과(알림, 화면 이동)는 지속 상태와 별도 effect stream으로 전달한다.

## 5. 핵심 도메인 모델

`Project`가 사용자가 반복해서 여는 중심 엔터티다. 세션은 항상 **server의 login shell**로 시작하고(local이든 SSH든 동일), AI CLI는 그 shell 안에서 세션 헤더의 agent 선택으로 실행·종료한다. 즉 CLI와 model은 세션에 고정되는 값이 아니라 언제든 바꿀 수 있는 실행 중 상태이며, CLI별 기본 model은 설정에서 정한다. "pin"은 server와 folder만 저장한다. `Host`는 앱이 실행 중인 로컬 컴퓨터 한 대를 가리키던 이전 개념을 대체한다 — 로컬이든 SSH로 연결된 원격이든 CLI를 실행할 수 있는 실행 대상은 모두 `Server` 하나로 표현하며, 과거의 `SshHost`/"SSH endpoint"는 `Server`의 `kind = ssh` 케이스로 흡수한다.

| 엔터티 | 주요 필드 | 설명 |
|---|---|---|
| `Server` | id, label, kind(`local`\|`ssh_config`\|`managed`), ssh_alias, capabilities | AI CLI를 실행할 수 있는 실행 대상. `local`은 앱이 실행 중인 호스트 자신, `ssh_config`는 `~/.ssh/config`의 wildcard가 아닌 `Host` alias, `managed`는 앱 안에서 직접 등록한 `ManagedServer`다. server 선택만으로 세션을 시작할 수 있어야 하며 경로를 미리 지정할 필요는 없다 — 최초 path는 항상 `~`로 시작한다(local은 실제 홈 디렉터리 절대 경로로 즉시 확정, ssh는 원격 파일시스템을 연결 전에 알 수 없으므로 로그인 셸의 `~`로 시작). |
| `ManagedServer` | id, name, host, port, username, auth(`password`\|`key_file`) | 앱 안에서 직접 등록한 원격 server. password는 OS keychain에만 저장하고 JSON 프로필에는 두지 않는다. key 인증은 기존 key file 경로 또는 앱이 생성한 ed25519 keypair 경로를 갖는다. |
| `Project` | id, display_name, canonical_path, trust_level, server_id | 사용자가 반복 작업하는 중심 단위 = server + folder. pin이 곧 Project 등록이다. CLI/model은 여기 저장하지 않는다. |
| `CliInstallation` | id, adapter_id, executable_path, version, status | 탐지된 CLI 설치본. Project는 탐지된 CLI 중 어떤 것이든 선택 가능한 목록으로 노출한다. |
| `Session` | id, project_id, server_id, working_directory, running_adapter_id, running_model, state, created_at, resume_token | server 위의 login shell 하나. `running_adapter_id`/`running_model`은 지금 그 shell 안에서 도는 AI CLI(없으면 null)로, 헤더의 agent 선택으로 실행하고 ■ 로 종료하면 shell로 돌아온다. 세션 이름은 폴더 버튼으로 folder를 바꿀 때 그 폴더 이름으로 바뀐다. |
| `Event` | id, session_id, seq, type, payload, raw_ref, created_at | 순서가 보장된 타임라인 항목 |
| `Approval` | id, session_id, kind, details, state, expires_at | 사용자 결정이 필요한 요청 |
| `Artifact` | id, session_id, kind, path, metadata | diff, 이미지, 보고서 등 산출물 |

Pin 동작: 세션을 pin하면 그 server와 folder가 저장된다. pin된 항목을 누르면 그 server의 그 folder에서 shell이 열리고, AI CLI는 열린 뒤 헤더에서 고른다.

### 5.1 세션 상태

```text
created -> starting -> running <-> waiting_for_input
                         |  \-> waiting_for_approval
                         |  \-> disconnected
                         +----> completed | failed | cancelled
```

- 상태 전이는 Rust 코어만 결정한다.
- UI 명령은 의도(command)이며 상태를 직접 변경하지 않는다.
- 모든 이벤트에는 세션 내 단조 증가 `seq`를 부여한다.
- 재연결 클라이언트는 마지막 `seq` 이후 이벤트를 요청한다.

## 6. CLI 어댑터 규격

각 어댑터는 다음 계약을 구현한다.

```rust
trait CliAdapter {
    fn metadata(&self) -> AdapterMetadata;
    async fn detect(&self) -> Vec<CliInstallation>;
    async fn capabilities(&self, install: &CliInstallation) -> Capabilities;
    async fn build_launch(&self, request: LaunchRequest) -> LaunchSpec;
    async fn parse(&mut self, chunk: RawChunk) -> Vec<NormalizedEvent>;
    async fn encode_input(&self, input: UserInput) -> EncodedInput;
    async fn encode_approval(&self, decision: ApprovalDecision) -> EncodedInput;
    async fn recover(&self, session: StoredSession) -> RecoveryPlan;
}
```

### 6.1 공통 capability

- `structured_output`
- `interactive_input`
- `resume_session`
- `approval_events`
- `file_diff_events`
- `tool_events`
- `token_usage`
- `model_selection`
- `attachments`
- `mcp_support`

### 6.2 공통 이벤트 타입

- `session.started`, `session.state_changed`, `session.ended`
- `message.user`, `message.assistant.delta`, `message.assistant.completed`
- `reasoning.summary` — CLI가 공개적으로 제공하는 요약만 저장
- `tool.started`, `tool.output`, `tool.completed`
- `file.changed`, `diff.available`, `artifact.created`
- `approval.requested`, `approval.resolved`
- `usage.updated`
- `process.stdout`, `process.stderr`, `process.exited`
- `error.adapter`, `error.process`, `error.protocol`

내부 추론 원문 또는 CLI가 제공하지 않는 비공개 정보의 추출을 시도하지 않는다.

### 6.3 프로세스 실행

- 지원 CLI는 플랫폼별 PTY에서 대화형 모드로 실행한다. pipe 기반 비대화형 실행은 자동화 기능에서만 별도로 사용한다.
- xterm의 `onData`를 PTY stdin에 그대로 전달하고 PTY stdout의 ANSI/VT 시퀀스를 xterm에 그대로 렌더링한다.
- Enter는 PTY의 carriage return으로 전달하며 ESC는 제품 정책에 따라 Ctrl-C(`0x03`)로 변환한다.
- 화면 크기 변경은 PTY resize로 전달하여 CLI 레이아웃을 즉시 다시 계산한다.
- 환경 변수는 allowlist 기반으로 전달하고 비밀 값은 로그에서 마스킹한다.
- 작업 디렉터리는 등록된 프로젝트 루트 또는 명시적으로 승인된 하위 경로여야 한다.
- 중단은 graceful interrupt → 제한 시간 → 강제 종료 순서로 처리한다.
- 앱 재시작 뒤 고아 프로세스를 식별하고 재연결 또는 안전 종료 선택지를 제공한다.
- 어댑터는 CLI 버전 범위를 선언하며 미지원 버전에서는 경고 후 Generic 모드로 내릴 수 있다.

## 7. 공통 프로토콜

### 7.1 전송과 직렬화

- 모바일은 SSH의 exec/subsystem channel 위에서 `fastade host --stdio`를 실행하고 newline-delimited JSON 메시지를 교환한다.
- SSH가 암호화, 서버 신원 확인, 사용자 인증을 담당하므로 앱 전용 TLS/페어링 프로토콜은 두지 않는다.
- 데스크톱 로컬 UI는 같은 command/event 의미론을 Tauri IPC로 사용한다.
- 대용량 바이너리: 별도 스트림 또는 서명된 일회성 다운로드 경로
- 모든 메시지: `protocol_version`, `message_id`, `timestamp`, `type`, `payload`
- 명령은 재시도에 안전하도록 `command_id` 기반 멱등성을 보장한다.
- 호스트는 연결 직후 지원 버전과 capabilities를 교환한다.

예시:

```json
{
  "protocol_version": "1.0",
  "message_id": "01J...",
  "timestamp": "2026-09-15T12:00:00Z",
  "type": "session.send_input",
  "payload": {
    "command_id": "01J...",
    "session_id": "01J...",
    "content": [{ "type": "text", "text": "테스트를 실행해줘" }]
  }
}
```

### 7.2 API 명령

- `host.get_info`, `host.list_cli_installations`
- `project.list`, `project.get`, `project.pin`, `project.unpin`
- `server.list`, `server.create`, `server.delete`
- `session.list`, `session.get`, `session.create`
- `session.send_input`, `session.cancel`, `session.subscribe`
- `session.get_events(after_seq)`
- `approval.resolve`
- `artifact.list`, `artifact.get`

모바일 MVP에서는 프로젝트 경로 추가, CLI 설치 경로 변경, 임의 셸 명령 실행을 허용하지 않는다.

## 8. 데스크톱 요구사항

### 8.1 주요 화면

- 온보딩: CLI 탐지, 프로젝트 등록, 보안 안내
- 홈: 프로젝트, 최근 세션, 실행 중 작업
- 새 세션: 버튼 두 개뿐이다. **Local**은 곧바로 이 컴퓨터의 `~`에서 `$SHELL -l` login shell을 연다. **Remote**는 server picker를 연다 — 왼쪽은 `~/.ssh/config`에서 읽은 Host 목록, 오른쪽은 앱에 등록한 server 목록이다. 오른쪽 `+`로 새 server를 등록한다: name, domain/IP, port, username, 인증(password 또는 key file). password는 OS keychain에만 저장하고(JSON 프로필에는 없음) `ssh` PTY에 password 프롬프트가 뜨는 순간 한 번 자동 입력한다. key file은 기존 파일을 고르거나 그 자리에서 ed25519 keypair를 새로 생성할 수 있으며, 생성 직후에만 공개키를 복사할 수 있는 1회성 패널을 보여준다(원격의 `authorized_keys`에 붙여넣는 용도). 어느 쪽을 고르든 그 server의 `~`에서 `ssh -tt`로 login shell이 열린다. CLI/model/path는 여기서 묻지 않는다.
- 세션 헤더: `⌂ path` 버튼(항상 표시; local은 OS 폴더 선택창, SSH는 원격 폴더 탐색기 → 고른 folder로 shell을 `cd`시키고 세션 이름을 그 폴더 이름으로 변경), 상태 오른쪽의 **AI 실행 선택**(Codex/Claude Code/Gemini — 설정의 CLI별 기본 model을 `--model`로 붙여 shell에 입력), ■(실행 중인 agent를 종료하고 shell로 복귀). agent 실행 중에는 folder 변경이 막힌다.
- shell의 cwd는 접속 시 한 번 설치하는 prompt hook(bash `PROMPT_COMMAND` / zsh `precmd`)이 OSC 7로 보고한다. 새 프롬프트가 그려졌다는 것은 agent가 종료됐다는 뜻이기도 하다.
- 세션: CLI-native xterm 터미널, 상태, 중지, 종료, 팝아웃
- 설정: CLI 설치, pin된 프로젝트, 정책, server(SSH) 목록, 데이터 관리
- 메인 윈도우에서는 세션을 동시 카드 그리드로 관리하고 원하는 세션을 별도 윈도우로 분리할 수 있다.
- 세션별 팝아웃 윈도우는 하나만 존재하며 이미 열린 세션은 새로 만들지 않고 기존 윈도우에 포커스한다.
- 동시에 열린 세션 팝아웃은 최대 10개로 제한하고, AI 세션 및 메인 윈도우의 카드 개수에는 이 제한을 적용하지 않는다.
- 메인 화면은 여러 CLI 세션을 동시에 확인할 수 있는 반응형 카드 그리드를 사용하고 장식성 여백을 최소화한다.
- 각 세션은 CLI, server, 작업 folder를 독립적으로 가지며 헤더에 항상 표시한다.
- drawer는 열고 닫을 수 있어야 하며 닫힌 상태에서는 세션 작업 영역이 전체 폭을 사용한다.
- command 입력에서 Enter는 즉시 전송하고 Shift+Enter만 줄바꿈으로 처리한다.
- ESC 키는 윈도우 닫기나 포커스 해제에 사용하지 않고 현재 선택된 AI 세션에 interrupt로 전달한다.
- 모든 세션 카드에는 AI 중지와 세션 종료 동작을 별도로 제공한다.
- 세션 이름은 server 이름으로 시작해 폴더 버튼으로 folder를 고를 때마다 그 폴더 이름으로 바뀐다. 우클릭 이름 변경도 가능하다.
- Pin은 server와 folder만 영속화한다. 설정(⚙)에 CLI별 기본 model 입력이 있다.
- 상단의 별도 workspace 제목 표시줄은 두지 않으며 세션이 하나면 해당 터미널이 남은 작업 영역 전체를 사용한다.

### 8.2 Tauri/Rust 경계

- WebView는 파일 시스템이나 프로세스 API에 직접 접근하지 않는다.
- Tauri command는 입력 스키마 검증, 인증/권한 검사 후 core use case만 호출한다.
- 장시간 스트림은 이벤트 채널을 사용하고 요청/응답 IPC를 점유하지 않는다.
- CSP를 엄격하게 적용하고 원격 웹 콘텐츠를 앱 권한 컨텍스트에서 로드하지 않는다.
- updater 서명 검증과 플랫폼별 코드 서명을 릴리스 필수 조건으로 둔다.

## 9. 모바일 요구사항

### 9.1 주요 화면

- SSH 호스트 등록 및 host key 확인
- 호스트/연결 상태
- 프로젝트 및 세션 목록
- 세션 타임라인과 입력 composer
- 승인 상세 및 승인/거부
- 알림/보안 설정

### 9.2 연결 모델

- 모바일에서 선택하는 원격 `Server`는 별도의 직접 TCP 연결이 아니라 SSH 접속 프로필이다. 같은 Wi-Fi의 PC도 hostname 또는 LAN IP를 SSH server로 등록한다.
- 사용자가 hostname, port, username과 인증 수단을 등록한다.
- 데스크톱과 모바일은 사용자의 `~/.ssh/config`에서 wildcard가 아닌 `Host` alias를 읽어 server 선택 목록으로 제공한다.
- SSH server의 최초 session folder는 원격 파일시스템을 연결 전에 탐색할 수 없으므로 `~`로 표시한다.
- 원격 folder가 확정되지 않은 `~` 세션은 CLI를 자동 실행하지 않고 `ssh -tt <Host>`로 login shell만 연다. 사용자가 원격에서 `cd` 후 원하는 CLI를 실행한다.
- login shell 접속 직후 감지되는 디렉터리(대개 원격 계정의 홈, 예: `/root`)는 사용자가 고른 값이 아니므로 세션 이름·경로에 자동 반영하지 않는다. UI는 감지된 경로를 정보로만 보여주고, 다음 두 경우에만 프로젝트 folder로 확정하고 세션 이름을 그 폴더 이름으로 바꾼다: (1) 사용자가 그 경로를 확인하는 버튼을 직접 누르거나, (2) 그 위치에서 AI CLI를 실제로 실행한다. 실행 전까지 사용자가 `cd`로 여러 폴더를 옮겨 다녀도 세션 이름이 계속 바뀌지 않는다.
- 향후 명시적인 원격 folder가 선택된 세션에서만 해당 folder로 이동한 뒤 선택한 AI CLI를 자동 실행한다.
- 인증은 SSH key를 기본으로 하고 password는 선택적으로 지원한다. 비밀은 모바일 OS 보안 저장소에 둔다.
- 최초 연결 시 SSH host key fingerprint를 사용자가 확인하고 이후 변경 시 연결을 차단한다.
- SSH 연결 후 `fastade host --stdio`를 실행하여 프로토콜 채널을 연다.
- 네트워크 전환/절전 후 지수 backoff로 SSH를 재연결하고 `last_seq`부터 동기화한다.
- 공인 인터넷 노출을 앱이 자동 구성하지 않는다. 사용자가 접근 가능한 SSH 호스트를 준비한다.
- PC와 폰 사이의 연결 세션은 일시적 transport일 뿐 영속 엔터티로 저장하지 않는다. AI 작업 `Session`은 PC의 SQLite에 독립적으로 유지되므로 폰 연결이 끊겨도 계속 실행된다.
- Flutter 구현은 `View → MobileViewModel → SessionClient → SshSessionClient`로 분리하며 View에서 SSH와 보안 저장소를 직접 호출하지 않는다.
- SSH host 메타데이터는 앱 설정 저장소에, password/key 같은 자격 증명은 OS 보안 저장소에 분리해 보관한다.

## 10. 보안 및 개인정보

### 10.1 위협 모델

- SSH 서버에 접근하는 비인가 클라이언트
- 악성 프로젝트가 프롬프트/출력으로 권한 상승을 유도하는 경우
- CLI 출력 또는 진단 로그를 통한 비밀 유출
- 심볼릭 링크를 이용한 프로젝트 경계 탈출
- 도난당한 모바일 기기 또는 유출된 SSH 키
- 손상되거나 사칭된 CLI 실행 파일

### 10.2 통제

- 앱은 별도 네트워크 포트를 열지 않으며 사용자가 관리하는 SSH 서버만 사용한다.
- SSH host key 검증을 필수로 하고 모바일 전용 SSH 키 사용을 권장한다.
- 비밀은 macOS Keychain, Windows Credential Manager, Linux Secret Service, iOS Keychain, Android Keystore에 저장한다.
- DB에는 자격 증명 원문 대신 secret reference만 저장한다.
- 프로젝트 canonical path 재검증과 symlink 경계 검사를 모든 파일 작업에 적용한다.
- 승인 유형을 `read`, `write`, `execute`, `network`, `secret_access`로 분류하고 위험 정보를 표시한다.
- 개인용 MVP에서는 별도 역할 관리, 생체 승인 정책, 감사 서버를 구현하지 않는다.
- 향후 배포 버전에서 다중 사용자 권한, 기기 폐기, 텔레메트리 정책을 별도 설계한다.

## 11. 저장소와 동기화

- SQLite + WAL 모드, 마이그레이션 버전 관리
- 이벤트는 append-only로 저장하고 파생 상태는 재구성 가능하게 한다.
- 원본 로그는 chunk 단위로 압축하고 최대 저장 용량을 사용자 설정으로 제공한다.
- MVP에서는 자동 보존 만료 정책을 두지 않고 사용자가 세션을 직접 삭제할 수 있게 한다.
- 모바일은 필요한 최근 이벤트만 암호화 로컬 캐시에 저장한다.
- 모바일에서 호스트 데이터 삭제 명령은 MVP에서 제외한다.

## 12. 오류 처리와 관측성

- 사용자 오류, 어댑터 호환성 오류, CLI 프로세스 오류, 호스트 연결 오류를 구분한다.
- 모든 오류는 안정적인 코드, 사용자 메시지, 복구 제안, correlation id를 가진다.
- 구조화 로깅을 사용하되 프롬프트/코드/토큰/환경 변수는 기본 필드에 넣지 않는다.
- 진단 번들은 앱/OS/CLI 버전, capability, 마스킹된 로그, DB 스키마 버전을 포함한다.
- 세션 프로세스 종료와 앱 crash 후 복구 여부를 측정한다.

## 13. 비기능 요구사항

| 항목 | MVP 기준 |
|---|---|
| 데스크톱 시작 | 일반 개발 머신에서 warm start 2초 이내 목표 |
| 스트림 지연 | 로컬 CLI 출력 → UI 표시 p95 100ms 이내 |
| 모바일 지연 | LAN에서 호스트 이벤트 → UI p95 300ms 이내 |
| 안정성 | 정상 종료된 이벤트의 유실 0건; 앱 crash 후 저장 이벤트 복원 |
| 확장성 | 호스트당 동시 실행 세션 4개, 보관 세션 10,000개 기준 검증 |
| 접근성 | 데스크톱 WCAG 2.2 AA 목표, 모바일 플랫폼 접근성 API 준수 |
| 국제화 | 문자열 외부화; 초기 언어 한국어/영어 |
| 지원 OS | 출시 시점 기준 각 플랫폼의 현재 및 직전 주요 버전 원칙 |

정확한 최소 OS 버전은 Tauri 2, Flutter 및 배포 스토어의 출시 시점 지원 정책을 확인해 릴리스 ADR에서 고정한다.

## 14. 테스트 전략

- Rust core 단위 테스트: 상태 전이, 정책, 경로 경계, 이벤트 순서
- 어댑터 golden test: CLI 버전별 캡처 출력 → 정규화 이벤트
- fake CLI 프로세스를 이용한 종료, hang, partial chunk, malformed JSON 테스트
- protocol contract test: Rust/TypeScript/Dart 동일 fixture 직렬화
- DB migration 및 crash recovery 테스트
- Tauri IPC 권한/입력 fuzz test
- 모바일 재연결, 중복 명령, 누락 이벤트 복구 테스트
- macOS/Windows/Linux 및 iOS/Android smoke test
- SSH host key 불일치, 인증 실패, 연결 중단 테스트
- Desktop/Mobile ViewModel의 상태 전이 및 effect stream 테스트

## 15. MVP 수용 기준

다음 조건을 모두 만족하면 MVP 기능 완료로 본다.

1. 지원 CLI 3종이 탐지되고 각각 새 세션을 시작할 수 있다.
2. 한 프로젝트에서 입력과 스트리밍 응답이 순서대로 표시된다.
3. 앱 재시작 후 완료 세션 이력과 원본 로그를 조회할 수 있다.
4. CLI가 요청한 명령/파일 쓰기를 UI에서 승인 또는 거부할 수 있다.
5. 세션 중단 시 자식 프로세스가 남지 않는다.
6. CLI의 비정상 종료가 앱 crash 없이 오류 상태로 변환된다.
7. 모바일에 SSH 호스트를 등록하고 세션 조회, 입력, 승인, 중단이 가능하다.
8. 연결이 끊겼다가 복구되어도 이벤트 누락/중복 표시가 없다.
9. SSH host key가 변경되면 명시적 재승인 전까지 연결되지 않는다.
10. 진단 번들에서 토큰, 키, 일반적인 비밀 패턴이 마스킹된다.
11. protocol fixture가 Rust, TypeScript, Dart에서 동일하게 통과한다.
12. View가 transport/storage API를 직접 참조하지 않는 아키텍처 검사가 통과한다.
13. 지원 대상 5개 OS의 배포 빌드와 최소 smoke test가 CI에서 통과한다.

## 16. 구현 단계

### Phase 0 — 검증 및 ADR

- 대상 CLI별 비대화형/구조화 출력, resume, 승인 방식 조사
- PTY와 pipe 비교 spike
- JSON Schema 대 protobuf 결정
- Flutter와 Rust 프로토콜 코드 생성 검증
- Flutter SSH channel과 `fastade host --stdio` prototype

### Phase 1 — Desktop Vertical Slice

- Rust core, SQLite, Generic 어댑터
- 프로젝트 등록 → 세션 실행 → 스트림 표시 → 종료
- Tauri UI 기본 화면과 오류 처리

### Phase 2 — 공식 CLI 어댑터

- Codex, Claude, Gemini 어댑터
- 승인, diff, usage, resume capability
- 버전 호환성 fixture/CI

### Phase 3 — Mobile Companion

- 공통 스키마 기반 Dart SDK
- SSH 호스트 등록, host key 검증, 재연결, 세션 UI
- 원격 입력, 승인/거부, 중단

### Phase 4 — Hardening & Release

- 보안 리뷰, 성능/복구 테스트, 접근성
- 코드 서명, 자동 업데이트, 스토어 배포
- 진단 및 사용자 문서

## 17. 주요 리스크와 대응

| 리스크 | 영향 | 대응 |
|---|---|---|
| CLI 출력/플래그의 잦은 변경 | 어댑터 파손 | 버전 범위, golden fixture, Generic fallback |
| PTY 출력 파싱의 불안정성 | 이벤트 오분류 | 구조화 모드 우선, 원본 보존, 보수적 파서 |
| 모바일 직접 실행 기대 | 제품 기대 불일치 | 호스트/컴패니언 모델을 온보딩과 문서에 명시 |
| 원격 승인 악용 | 코드/데이터 손상 | 명령 상세 표시, SSH key 분리, 향후 배포 전 권한 정책 추가 |
| 앱 종료 후 프로세스 고아화 | 리소스/보안 문제 | process group/job object 관리, startup reconciliation |
| 플랫폼별 프로세스 차이 | 일정 증가 | runtime 추상화와 OS별 통합 테스트 |
| 여러 CLI 라이선스/약관 차이 | 배포 제한 | 바이너리 번들 금지, 사용자 설치본 연동, 출시 전 검토 |

## 18. 확정 사항과 남은 결정

### 18.1 확정 사항

- 제품명: `fastade`
- 데스크톱 앱 프레임워크: Tauri 2 + Rust
- 데스크톱 View: Svelte 5 + TypeScript + Vite
- 모바일: Flutter
- 기본 CLI: Codex CLI, Gemini CLI, Claude Code
- 모바일 원격 접속: SSH
- 초기 사용자 모델: 단일 사용자
- 자동 데이터 보존 정책: MVP에서 제외

### 18.2 Phase 0에서 결정할 항목

1. 프로토콜 스키마(JSON Schema + 코드 생성 또는 protobuf)
2. 지원 CLI의 정확한 최소 버전
3. SSH transport는 `dartssh2`, 자격 증명 저장은 `flutter_secure_storage`를 사용한다. 최초 vertical slice는 password 인증을 제공하고 전용 private key 인증을 추가한다.
4. 푸시 알림 전달 방식
5. 현재 데스크톱 vertical slice(`apps/desktop`)는 Section 5의 `Server`/`Project` 모델을 아직 구현하지 않았다 — CLI·model·SSH endpoint·path를 하나로 묶은 flat한 `SavedSession` 프로필만 존재한다. `SavedSession`을 Project 중심 모델(Project는 고정, server·CLI·model은 세션 생성 시 자유 선택, pin은 Project의 default 저장)로 마이그레이션하는 작업과, 그때 기존에 저장된 `saved-sessions.json`을 깨지 않고 옮기는 방법을 Phase 1~2 사이에서 별도로 설계한다.

## 19. Definition of Done

기능은 코드 작성만으로 완료되지 않는다. 다음을 모두 충족해야 한다.

- 명세된 수용 기준과 자동화 테스트 통과
- 오류/취소/재연결 경로 검증
- 보안 및 개인정보 체크리스트 검토
- 한국어/영어 UI 문자열과 접근성 레이블 제공
- 관련 protocol/schema 및 사용자 문서 갱신
- 지원 플랫폼 배포 빌드 검증
