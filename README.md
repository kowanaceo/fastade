# fastade

Codex CLI, Claude Code, Gemini CLI 세션을 한곳에서 실행하고 관리하기 위한 데스크톱·모바일 클라이언트입니다.

> 이 프로젝트는 현재 초기 개발 단계입니다. 데이터 형식과 기능이 예고 없이 바뀔 수 있으며, 모바일의 실제 원격 세션 연결은 아직 완성되지 않았습니다.

## 다운로드

- **macOS (Apple Silicon)**: [최신 릴리즈에서 `.dmg` 다운로드](https://github.com/kowanaceo/fastade/releases/latest)
- 앱이 서명/공증되지 않아 처음 실행 시 Gatekeeper가 막을 수 있습니다. dmg를 마운트한 뒤 Finder에서 `fastade.app`을 우클릭 → 열기로 실행하세요.
- Intel Mac, Windows, Linux 빌드는 아직 제공하지 않습니다.

## 주요 기능

- 로컬 또는 SSH 서버의 터미널 세션 생성·복원·관리
- Codex CLI, Claude Code, Gemini CLI 실행과 기본 모델 설정
- 세션별 터미널 팝아웃과 작업 상태·메모리 사용량 표시
- Codex와 Claude Code 사용량 조회
- 선택형 읽기 전용 MCP 연동을 통한 전체 세션 상태 조회
- Flutter 모바일 클라이언트의 SSH 프로필 및 보안 저장소 지원

## 요구 사항

- Node.js 22.12 이상(또는 24 이상)과 npm
- Rust 1.88 이상
- 데스크톱 앱 빌드에 필요한 [Tauri 2 시스템 의존성](https://v2.tauri.app/start/prerequisites/)
- 모바일 개발 시 Flutter SDK와 Android Studio 또는 Xcode
- 사용할 AI CLI(Codex CLI, Claude Code, Gemini CLI)는 별도 설치 및 로그인이 필요합니다.

현재 MCP 브리지의 로컬 IPC는 macOS와 Linux에서만 동작합니다. Windows용 named pipe 지원은 아직 구현되지 않았습니다.

## 데스크톱 개발

```bash
npm install
npm run check
npm run tauri:dev
```

프로덕션 번들은 다음 명령으로 만듭니다.

```bash
npm run tauri:build
```

브라우저 단독 실행은 지원하지 않습니다. 프런트엔드는 Tauri IPC를 통해 Rust host와 통신하므로 `npm run tauri:dev`로 실행해야 합니다.

## 모바일 개발

```bash
cd apps/mobile
flutter pub get
flutter analyze
flutter test
flutter run
```

모바일 앱은 CLI를 직접 실행하지 않고 SSH로 데스크톱에 접속하도록 설계되어 있습니다. SSH 프로필, OS 보안 저장소의 암호 보관, host key 최초 승인 및 변경 차단, 원격 세션 UI까지 구현되어 있으며 `fastade host --stdio` 브리지는 아직 개발 중입니다.

## AI CLI 연동과 보안

설정에서 MCP 연동을 켜면 fastade는 해당 CLI의 사용자 설정 파일에 `fastade_mcp` 항목을 추가합니다.

| CLI | 변경되는 사용자 설정 | 상태 감지 |
| --- | --- | --- |
| Codex | `~/.codex/config.toml`, `~/.codex/hooks.json` | 턴·도구 실행, 승인 요청, 중단/완료 lifecycle hook과 turn-complete 알림 |
| Claude Code | `~/.claude.json`, `~/.claude/settings.json` | 세션·턴·도구 실행, 승인 요청, 완료/실패 및 사용자 입력 알림 hook |
| Gemini CLI | `~/.gemini/settings.json` | 터미널 출력 기반 추정 |

fastade가 만든 항목만 제거하며 기존 사용자 설정은 유지합니다. Codex는 새 hook을 바로 실행하지 않을 수 있습니다. Codex에서 `/hooks`를 열어 fastade가 추가한 hook을 검토하고 신뢰해야 작업 중/대기 상태가 정확히 표시됩니다.

SSH 비밀번호는 OS 보안 저장소에 저장됩니다. 저장소에 인증정보, 개인키, `.env`, 서명 키를 커밋하지 마세요. 이미 커밋한 비밀정보는 `.gitignore`만으로 제거되지 않으므로 키를 폐기·재발급하고 Git 기록에서도 삭제해야 합니다.

## 프로젝트 구조

```text
apps/desktop/  Svelte 5 + TypeScript + Tauri 2 데스크톱 앱
apps/mobile/   Flutter 모바일 앱
SPEC.md        제품 및 UX 명세
```

## 검사

```bash
npm run check
cd apps/desktop/src-tauri && cargo test
cd apps/mobile && flutter analyze && flutter test
```

## 기여

이슈와 pull request를 환영합니다. 큰 변경은 구현 전에 이슈에서 범위와 방향을 먼저 논의해 주세요. 변경 사항에는 관련 테스트를 포함하고 위 검사 명령을 통과해야 합니다.

## 라이선스

아직 라이선스가 지정되지 않았습니다. 라이선스 파일이 추가되기 전까지는 저작권자의 명시적 허가 없이 이 코드를 복제·수정·배포할 수 없습니다.
