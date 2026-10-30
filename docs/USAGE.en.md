# fastade User Guide

*[한국어](USAGE.ko.md)*

fastade is a desktop terminal client for running and managing Codex CLI, Claude Code, and Gemini CLI sessions in one place. Pick a local or SSH server and a login shell opens at that server's home directory; point it at a folder, then pick the AI CLI you want from the session header and run it right there in that shell.

It renders the raw PTY screen as-is with xterm and only layers on extras like activity status, memory usage, and usage stats — so you keep each CLI's native features (slash commands, approval prompts, interactive UI) while managing multiple projects and sessions side by side.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| ⌘/Ctrl + Tab (Shift for reverse) | Cycle through open sessions |
| ⌘/Ctrl + = (or +) | Increase terminal font size |
| ⌘/Ctrl + - (or _) | Decrease terminal font size |
| ⌘/Ctrl + 0 | Reset font size to default |
| Esc | Context-dependent: cancel a group rename → close an open context menu → close settings → interrupt the running agent in the selected session |

Matched top to bottom, most specific first — pressing Esc while renaming a group only cancels the rename, not the settings panel.

## Key features

**Server picking and folder targeting** — Pressing ▶ opens a login shell at the home (`~`) of a local or registered SSH server. The folder button (⌂) in the session header lets you pick a working folder; the shell `cd`s there and the session is renamed after that folder.

**Pinning and profiles** — Pinning a server+folder combination lets you reopen that exact spot instantly next time. You can also drag a session from the sidebar's "Ungrouped" section into a group.

**Session groups** — Create new groups in the sidebar and drag-and-drop sessions to organize them by project. Deleting a group moves its sessions back to the default group.

**Running AI agents** — Pick Codex/Claude Code/Gemini from the session header's agent picker to run it inside that shell (using the per-CLI default model set in Settings). Model suggestions are discovered from each CLI's local cache, while the field still accepts any model ID. Pressing ■ exits the agent and returns to the shell. fastade retains the last model used by each CLI; when lifecycle integration is enabled, it also records the provider session ID and effective model reported by the CLI transcript.

**Activity status badges** — A colored dot on each session card shows Working / Needs you (waiting on approval or input) / Ready at a glance. Codex and Claude Code report status through their own CLI hooks for higher accuracy; Gemini and remote SSH sessions are inferred from terminal output patterns.

**Memory and usage display** — Session cards show the resident memory used by that shell and its child processes, and the AI USAGE panel at the top of the sidebar checks Codex and Claude usage every 5 minutes. Codex reuses session logs less than 5 minutes old and queries account limits directly when logs are older or a window has reset. Claude's limits are read through its `/usage` screen and cached for 10 minutes. The refresh button bypasses both agents' caches and shows `…` while checking. Each agent's result appears as soon as it arrives. SSH hosts with a live session show their Codex limits below the agent, and when signed in, usage from your other devices appears there too.

**MCP integration** — Turning on the toggle in Settings adds a `fastade_mcp` entry to that CLI's own config, letting the agent read fastade's session state (`list_sessions`, `list_projects`) and read, create, edit and delete the notes and tasks of your saved sessions (`list_records`, `create_record`, `update_record`, `delete_record`). Global Notes (Settings → Manage) are notes that apply to every project: each one is automatically included as MCP server instructions whenever a new agent context starts. Save changes, then start or resume an agent to apply them; an already-running context keeps what it started with. They are ordinary notes under the reserved `profile_id` `global`, so agents manage them with the same four record tools, and they cannot be tasks. Session control is never exposed. SSH sessions work only while you are signed in to fastade: fastade forwards its local socket into the session and installs a small Python 3 relay (`~/.fastade/bin/fastade-mcp`), so no account token or credential is ever stored there. Only entries fastade created are ever removed; your existing config is left untouched. For Codex, open `/hooks` and trust the hook fastade added, or working/waiting status won't display accurately.

**Pop-out and reconnect** — The ↗ button on a session card pops it into its own window. Session history and metadata are restored even after closing and reopening the app, and a closed session can be reconnected with ↻.

**Desktop tunnel (macOS)** — In Settings → Desktop tunnel, save the backend SSH host and dedicated key, then enable the toggle. Sign-in and Remote Login (sshd) on the Mac are required. The server-assigned backend localhost port forwards to the Mac's SSH service. The app reconnects with backoff, prevents idle sleep while connected, and stops/reports off on OFF, logout or app exit. See [setup and verification](DESKTOP-TUNNEL.md). The mobile jump connection and sharing an existing terminal screen are separate features.

## Tips and troubleshooting

**Run tasks** — Each **To do** task has a ▶ button. The ▶ immediately left of **Tasks → +** runs all current to-do tasks for that session together, in display order. The request goes to the AI in the session where you opened the task panel. If the shell has no AI running, fastade starts the most recently used enabled CLI, or the first enabled CLI, using its default model setting. A running AI must show **Ready** before submitting tasks. Selected tasks change to **In progress**; a failed handoff restores unchanged task statuses. When fastade MCP record tools are available, the executing agent updates tasks to **Done** after implementing and verifying them. Enable the CLI's MCP integration to allow those completion updates. See [task execution details](TASK-EXECUTION.md).

**Voice input** — Sign in, then click the microphone in a connected session's header to start recording after granting microphone permission. Record up to 60 seconds, then stop to transcribe and automatically send the recognized text to that specific session; line breaks become spaces. The last selected speech language (automatic detection, Korean, or English) is remembered. If sending fails, review/edit the text and press **Send to session** to retry. Cancel releases the microphone and cancels any active transcription request. A failed transcription can retry the same audio while the dialog stays open. Recordings are held in memory and discarded when the dialog closes. This requires the aisshapi `/stt/transcribe` endpoint; a server without it displays an availability error. See [implementation and verification notes](VOICE-INPUT.md).

- If Codex's working/waiting status looks wrong or stuck, check in Codex that you trusted the hook fastade added via `/hooks` — without it, status falls back to guessing from terminal text, which is less accurate.
- Gemini CLI and remote SSH sessions have no hook integration and infer status from terminal output alone; leftover text on screen can make the status briefly wrong.
- SSH passwords are stored only in the OS's secure keychain, never in the profile JSON.
- The MCP bridge's local IPC currently only works on macOS and Linux; Windows isn't supported yet.
- Standalone browser use isn't supported — the frontend talks to the Rust host over Tauri IPC, so run it with `npm run tauri:dev` (or a built app).
