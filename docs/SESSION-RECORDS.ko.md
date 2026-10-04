# 세션 노트·task 구현 상태와 결정 사항

## 확정한 모델과 구현

- 노트: `kind: note`, `title`, `content`.
- Task: `kind: task`, `title`, `content`, `status: todo | in progress | done`.
- 제목은 공백만 입력할 수 없다. 내용은 빈 문자열을 허용하고 줄바꿈·들여쓰기를 보존한다.
- 소속은 재접속 때 바뀌는 PTY ID 대신 기기 간 동기화되는 saved profile의 `profileId`를 사용한다.
- `SessionRecords.svelte`는 목록, 생성, 수정, 삭제와 task 상태 편집을 제공한다. 저장 실패 때 입력을 유지하며, 저장 성공 응답을 받은 뒤 목록에 반영한다.
- `SessionRecordRepository`는 백엔드 CRUD 연결 경계다. 컴포넌트에는 실제 구현을 주입하고 profileId별로 새 인스턴스를 마운트해야 한다.

**현재 앱 화면에는 연결하지 않았다.** 백엔드 소스와 API 계약이 확인되지 않아 HTTP adapter, 저장, 동기화, MCP CRUD는 구현하지 않았다. 로컬 저장을 백엔드 동기화처럼 표시하지 않는다.

## 현재 MCP·인증 구조

- 설정의 MCP 토글은 로컬 사용자 설정만 수정한다: `~/.claude.json`, `~/.gemini/settings.json`, `~/.codex/config.toml`.
- 이 설정은 로컬 `fastade_mcp` stdio 바이너리를 실행한다. 바이너리는 로컬 Unix socket으로 실행 중인 데스크톱 앱에 접근한다.
- 도구는 `list_sessions`, `list_projects`이며 노트·task CRUD는 아직 없다.
- 새 managed remote를 생성해도 MCP를 원격에 설치·등록하는 코드는 없다.
- 데스크톱 aisshapi 인증은 access/refresh token을 Keychain에 보관한다. refresh token을 원격에 복사하는 기능은 없다.

## 백엔드 확인 후 진행할 작업

1. aisshapi의 저장 모델과 CRUD·동기화 계약 확인. 기존 sync의 허용 entity type에 note/task가 포함되는지, 상태 표현과 버전·충돌·삭제 처리를 결정한다.
2. 직접 편집과 MCP가 같은 백엔드 서비스와 소유권 검사를 사용하도록 연결한다. 저장 실패·동기화 충돌은 성공으로 표시하지 않는다.
3. aisshapi 아래 HTTP MCP endpoint와 도구 등록을 구현한다. 최종 URL은 백엔드 라우팅 확인 후 확정한다. 기존 REST sync endpoint는 그대로 MCP endpoint가 되지 않는다.
4. 노트·task 변경을 기기 간에 반영하고, 재시작·재접속 후 같은 profile에서 복원하는지 검증한다.
5. 승인된 인증 방식으로 SSH remote의 CLI별 MCP 설정을 등록한다. 기존 사용자 설정 보존, 재시도 시 중복 방지, 설치 실패 표시와 서버 삭제 시 인증 폐기를 다룬다.

## 사용자 결정이 필요한 항목

- **백엔드 위치**: aisshapi 저장소 또는 접근 가능한 경로.
- **원격 인증**: 서버별 전용 토큰 발급/폐기 또는 원격 CLI에서 OAuth 인증. 전용 토큰을 선택하면 범위(계정 전체/선택한 profile), 만료·재발급 정책도 확정해야 한다.
- **자동 등록 범위**: 현재 로컬에서 켠 CLI만 새 remote에 등록할지. 신규 remote에만 적용할지, 기존 remote도 적용할지. CLI가 설치되지 않은 remote에서 CLI 자체까지 설치할지는 별도 결정한다.
- **Profile 삭제 정책**: 연결된 노트·task도 삭제할지, 보관할지.

HTTP MCP를 쓰면 원격에는 데스크톱 stdio 바이너리를 복사할 필요 없이 CLI에 URL과 인증을 등록할 수 있다. 연결 등록 자동화와 첫 인증 자동화는 별개다.

MCP HTTP 인증의 OAuth discovery 방식은 [공식 MCP authorization 문서](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/draft/basic/authorization/index.mdx), Gemini의 HTTP/OAuth 설정 지원은 [공식 Gemini CLI 문서](https://geminicli.com/docs/tools/mcp-server/)를 참고했다. 서버별 전용 토큰은 이 프로젝트에 대한 제안이며 확정된 프로토콜 구현이 아니다.
