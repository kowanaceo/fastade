# AI usage collection and upload

Contract for task `e0222f57-5376-4f03-8665-edac63546f1b`.

`PUT /usage/snapshots/{agentId}/{hostId}` receives:

```json
{
  "agentId": "claude",
  "hostId": "device-example",
  "hostLabel": "This device",
  "accountId": "provider-account-uuid",
  "accountEmail": "user@example.com",
  "collectedAt": 1792683943,
  "windows": [
    {"label": "5h", "remainingPercent": 79, "resetsAt": 1792687140},
    {"label": "Week", "remainingPercent": 88, "resetText": "Resets after a provider event"}
  ]
}
```

## Reset fields

- `resetsAt` is an **integer Unix epoch timestamp in seconds**, UTC. Convert to a display date with `new Date(resetsAt * 1000)`. Codex log/app-server values are normalized to integer seconds; millisecond numeric inputs are divided by 1000.
- Relative descriptions such as `Resets in 2h 13m` are resolved once against the original collection time. Short and spelled-out day/hour/minute/second units are supported. Upload delays and cache reads do not move the reset.
- Claude clock/date descriptions use the printed IANA timezone, such as `Asia/Seoul`, including daylight saving rules. The system timezone is used only when no timezone was printed. A date without a year uses the collection year; a clock without a date uses the collection date in that timezone. Elapsed dates/times are never advanced to another day, week or year.
- If parsing cannot determine an instant, preserve the entire reset description in `resetText`; do not truncate it to eight words. Unknown timezones, ambiguous/nonexistent DST times, malformed or overflowing durations also use this fallback.
- A known `resetsAt` takes precedence over `resetText`. Unknown reset fields are **omitted**, never sent as `null`. If neither field is known, omit both.
- Expired timestamps from logs and explicit provider data remain expired. Local Codex/Claude caches with an expired window trigger a fresh collection; remote Codex log snapshots retain the expired instant for consumers to recognize.

## Collection time and account identity

`collectedAt` is integer epoch **seconds**, independent of `resetsAt`:

- Codex log snapshots use the original `token_count` event timestamp, including over SSH. Logs without a valid timestamp cannot be uploaded as freshly collected data.
- Live Codex app-server queries use response receipt time; Claude uses the time its usage output was captured, before process cleanup.
- Cached results preserve that original timestamp. The frontend copies it into the snapshot; waiting for another agent, remote hosts, account lookup or upload does not restamp it. Results without a collection timestamp are not uploaded.
- `accountId` is the provider identity, unchanged across hosts: Codex `tokens.account_id` in `auth.json`, Claude `oauthAccount.accountUuid` in `.claude.json`. It is captured with local collection results, not attached later from a potentially different login. If the login changes during collection, discard the result and require another refresh. Claude's cache is scoped to the account.
- Upload tracking includes account identity and both reset fields. Switching accounts or changing only fallback text triggers an upload even when percentages match. Older source timestamps cannot replace this client's newer upload. Periodic reuploads retain the original collection timestamp.

The server selects the newest `collectedAt` per agent/account. The sidebar heading clock is separate from these collection timestamps.

## Verification

Automated checks:

```sh
npm run test:usage
npm run check
npm run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
rustfmt --edition 2021 --check apps/desktop/src-tauri/src/usage.rs
```

Tests cover epoch normalization, relative-time anchoring, timezone/date parsing, elapsed resets, DST ambiguity, complete fallback text, omitted null fields, cache timestamps, account changes, and upload after a slow peer collector.

Opt-in live checks require installed, signed-in CLIs and network/keychain access:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml usage::tests::fetches_live_codex_usage -- --ignored --nocapture
FASTADE_DEBUG_USAGE=1 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml usage::tests::fetches_live_claude_usage -- --ignored --nocapture
FASTADE_USAGE_VERIFICATION_JSON=/tmp/verified-usage.json cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml usage::tests::uploads_live_usage_and_verifies_server_round_trip -- --ignored --nocapture
```

The last check uploads real freshly collected limits to the desktop's existing device row using its signed-in backend session, then verifies GET returns identical windows/account/time. The optional JSON output contains snapshots only, never authentication tokens. `FASTADE_DEBUG_USAGE` logs the CLI screen, so keep that output local.

Live verification on 2026-10-23 (KST) matched these source reset times:

| Agent | Window | Epoch seconds | KST |
| --- | --- | --- | --- |
| Claude | 5h | 1792687140 | 2026-10-23 01:39:00 |
| Claude | Week | 1793012340 | 2026-10-26 19:59:00 |
| Codex | 5h | 1792697579 | 2026-10-23 04:32:59 |
| Codex | Week | 1793105528 | 2026-10-27 21:52:08 |

Claude's actual `/usage` output printed `Resets 1:39am (Asia/Seoul)` and `Resets Oct 26 at 7:59pm (Asia/Seoul)`. Codex's actual `/status` screen printed `5h limit ... resets 4:32 AM` and `Weekly limit ... resets 9:52 PM on 27 Oct`, matching its live account rate-limits response to the displayed minute.

Both uploads passed authenticated PUT/GET round-trip checks. Immediate read-only inspection of aisshapi `usage_snapshots.windows_json` matched both wire payloads exactly, including original `collectedAt`, provider account identity, and omitted null reset fields. The verified collection timestamps were 1792684360 (Codex) and 1792684363 (Claude). Separately reading provider identities on backend and kowanasdev confirmed the same Codex account ID (`37ec7131-…`) on both hosts.

Validation passed: 34 JavaScript tests; 64 Rust library tests and 6 MCP binary tests (6 environment-dependent tests excluded from the default run, with the two collectors and upload round trip run explicitly); Svelte check with zero errors/warnings; Vite production build; formatting of the modified Rust file; and `git diff --check`. Vite retains its existing large-bundle advisory.

These checks exercise the modified collectors and upload code. An already-running older desktop process continues using its loaded code and may overwrite the same device rows until the rebuilt app is restarted.

## Provider email upload

Both aissh and aicli attach optional `accountEmail` alongside `accountId`. Codex decodes only the `email` claim of `tokens.id_token`; Claude reads `oauthAccount.emailAddress`. The email is captured with the account before collection and validated again afterward, including on Claude cache hits and remote Codex probes. A later local-account lookup must not relabel an earlier sample. Missing/unreadable emails are omitted; credentials and JWTs are never included. Changing only the email also triggers an upload.

Backend follow-up task: `770d693d-08e2-4118-9b65-49d105930ea8` on aisshapi. Its request model must accept `accountEmail` before these new payloads are accepted (older servers reject it with 422); it owns database persistence, GET round trips and grouping by provider account. No backend code or deployment is part of this client change.
