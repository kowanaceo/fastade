# Task execution from session records

Each task whose status is `todo` has a play button beside its status. The play button directly left of the Tasks add button selects all current todo tasks belonging to that saved profile. Notes, completed tasks, tasks already in progress, and other profiles are excluded. Global Notes has no task execution controls.

Opening the task panel pins its runtime session ID as well as its saved profile ID. Changing the selected terminal never redirects a request. A disconnected or mismatched session cannot receive tasks. Running agents must be Ready: task text is not sent into a working agent or its approval prompt. When no AI is running, the request starts the most recently used enabled CLI, falling back to the first enabled CLI and respecting its default model setting. Initial prompts are positional arguments for Codex and Claude; Gemini uses [`--prompt-interactive`](https://geminicli.com/docs/cli/cli-reference/).

The backend claims todo tasks under the existing record-store lock and updates their status to `in progress` in one file write. Individual play filters by the selected ID; run-all reads current persisted todos, including ones added after the panel originally loaded. Claimed tasks are ordered by creation time, matching the record list. Another window cannot claim those same tasks again. Both claim and restore emit `records-changed` for other windows and sync.

The agent receives one prompt containing the selected record IDs, titles, and complete requirements as JSON. Newlines and terminal controls are escaped while preserving the decoded content. A new CLI's prompt and model are shell-quoted as single arguments. The prompt and one Enter are submitted together through the existing terminal-write queue.

The request asks the AI to implement and verify each listed task, and to use MCP `update_record` with exact IDs to set `done` only after actual completion. Blocked tasks remain in progress, with their remaining work recorded, and the agent continues with the other listed tasks. Completion updates require the CLI's fastade MCP tools to be enabled and present in its current context. A successful PTY handoff itself never marks tasks done.

If submission fails, only records still identical to their claimed snapshot are restored to todo. A concurrent edit or completion is preserved. Errors remain visible in the panel during sync refresh; a failed rollback is explicitly reported. As with other PTY writes, a write failure can be partial, so the error asks the user to check the terminal before retrying.

Verification:

```sh
npm run check
npm run test:tasks
npm run build
cd apps/desktop/src-tauri && cargo test
```

Task tests cover individual/batch scope, one handoff per batch, prompt formatting and shell arguments, empty/stale selections, rollback failures, the compiled Svelte viewmodel's existing-agent and auto-launch flows, pinned destinations, working/approval/disconnected states, and duplicate clicks. Rust tests cover atomic claims, exclusion of other profiles and statuses, duplicate claims, display order, and rollback after concurrent edits/completion. Viewmodel tests use Vite in middleware mode with WebSockets disabled; they do not launch actual AI processes or execute saved user tasks.
