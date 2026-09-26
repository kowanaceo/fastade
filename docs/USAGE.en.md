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

**Running AI agents** — Pick Codex/Claude Code/Gemini from the session header's agent picker to run it inside that shell (using the per-CLI default model set in Settings). Pressing ■ exits the agent and returns to the shell.

**Activity status badges** — A colored dot on each session card shows Working / Needs you (waiting on approval or input) / Ready at a glance. Codex and Claude Code report status through their own CLI hooks for higher accuracy; Gemini and remote SSH sessions are inferred from terminal output patterns.

**Memory and usage display** — Session cards show the resident memory used by that shell and its child processes, and the AI USAGE panel at the top of the sidebar lets you check Codex and Claude usage on a rolling basis.

**MCP integration** — Turning on the toggle in Settings adds a read-only `fastade_mcp` entry to that CLI's own config, letting the agent read fastade's full session state (`list_sessions`). Only entries fastade created are ever removed; your existing config is left untouched. For Codex, open `/hooks` and trust the hook fastade added, or working/waiting status won't display accurately.

**Pop-out and reconnect** — The ↗ button on a session card pops it into its own window. Session history and metadata are restored even after closing and reopening the app, and a closed session can be reconnected with ↻.

## Tips and troubleshooting

- If Codex's working/waiting status looks wrong or stuck, check in Codex that you trusted the hook fastade added via `/hooks` — without it, status falls back to guessing from terminal text, which is less accurate.
- Gemini CLI and remote SSH sessions have no hook integration and infer status from terminal output alone; leftover text on screen can make the status briefly wrong.
- SSH passwords are stored only in the OS's secure keychain, never in the profile JSON.
- The MCP bridge's local IPC currently only works on macOS and Linux; Windows isn't supported yet.
- Standalone browser use isn't supported — the frontend talks to the Rust host over Tauri IPC, so run it with `npm run tauri:dev` (or a built app).
