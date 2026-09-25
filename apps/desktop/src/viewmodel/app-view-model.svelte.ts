import type { DesktopClient, SshHost, TerminalEvent } from '../application/desktop-client';
import { type AgentUsage, type CliKind, type CliSessionSummary, type CreateServerInput, type ManagedServer, type SavedSessionProfile, type SessionSummary, type UpdateServerInput, type WebAgent } from '../domain/session';

type TerminalSink = (data: string, replay?: boolean) => void;

export interface SessionGroup {
  id: string;
  name: string;
  custom: boolean;
  sessions: SessionSummary[];
  entries: SessionGroupEntry[];
}

export interface SessionGroupEntry {
  id: string;
  sessionId?: string;
  profileId?: string;
  title: string;
  endpoint: string;
  projectPath: string;
  status: SessionSummary['status'] | 'disconnected';
  cli?: CliKind;
  activity?: AgentActivity;
}

/** Heuristic classification of what a running agent is doing right now,
 * inferred from its raw PTY output — never a value the CLI reports itself.
 * `working`: actively producing output (spinner frames, "esc to interrupt").
 * `waiting`: an approval/confirmation prompt requires the user's attention.
 * `idle`: the agent is at its composer with no work in progress, or no agent
 * is running in this session's shell right now. */
export type AgentActivity = 'working' | 'waiting' | 'idle';

interface GroupDefinition { id: string; name: string; }
interface ShellInputActions { launchedAgent: CliKind | null; }

// Spinner frames used by ora-style CLI progress indicators (Codex, Claude
// Code, Gemini CLI all use Braille-pattern spinners) plus the phrases these
// tools print alongside a busy spinner.
const BUSY_PATTERN = /[⠀-⣿]|(?:esc|ctrl\+c) to interrupt|↑\s*\d|thinking…|generating…|running…|working(?:…|\s*\()|combobulating|reticulating/i;
// Confirmation/approval prompts, which always mean the agent is blocked on
// the user regardless of how recently output arrived. Selection menus (e.g.
// Claude Code's AskUserQuestion footer "Enter to select · ↑/↓ to navigate")
// count too: remote SSH sessions have no lifecycle hooks, so this text is the
// only signal that the agent is waiting on a choice.
const WAITING_PATTERN = /\(y\/n\)|\[y\/n\]|do you want to (proceed|continue|allow)|press enter to (continue|confirm)|enter to select · ↑\/↓ to navigate|approve this|allow this (command|tool|action)|grant (access|permission)|❤ waiting for/i;
// Codex keeps "context left" and "? for shortcuts" visible while it is
// working too, so neither is an idle signal. The completed-turn summary is
// specific to the point at which the composer becomes ready again.
const CODEX_IDLE_PATTERN = /\bworked for\b[^\r\n]*\b(?:done|completed)\b/i;
// If a working agent produces no output at all for this long, assume it
// finished its turn and is now idling on an input prompt.
const AGENT_IDLE_TIMEOUT_MS = 1800;
// How much recent printable PTY output to keep per session for cue ordering.
const ACTIVITY_TAIL_CHARS = 4000;

/** Removes terminal protocol/control bytes while retaining the text the TUI
 * painted. Codex emits cursor and device-status traffic even on an idle
 * composer; those bytes must not count as active work. */
function printableTerminalText(value: string): string {
  return value
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, '')
    .replace(/\x1b[P^_][\s\S]*?\x1b\\/g, '')
    .replace(/\x1b\[[0-?]*[ -\/]*[@-~]/g, '')
    .replace(/\x1b[@-_]/g, '')
    .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, '');
}

function lastPatternIndex(value: string, pattern: RegExp): number {
  const matches = value.matchAll(new RegExp(pattern.source, `${pattern.flags.replace('g', '')}g`));
  let last = -1;
  for (const match of matches) last = match.index;
  return last;
}

const UNGROUPED_ID = 'ungrouped';
const DEFAULT_GROUP_NAME = 'Sessions';
const GROUPS_STORAGE_KEY = 'fastade.session-groups.v1';
const WEB_AGENTS_STORAGE_KEY = 'fastade.web-agents.v1';
const AGENT_MODELS_STORAGE_KEY = 'fastade.agent-default-models.v1';
const FONT_SIZE_STORAGE_KEY = 'fastade.terminal-font-size.v1';
const DEFAULT_FONT_SIZE = 11;
const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 28;
// Installed once per session shell (local or SSH). Rather than typing a
// visible probe command after every `cd`, this hooks the shell's own prompt
// (bash PROMPT_COMMAND / zsh precmd) to silently report its cwd via OSC 7 on
// every prompt redraw — the same mechanism VS Code's Remote-SSH and iTerm2
// shell integration use. A fresh prompt also means "no agent is running".
// Uses a private OSC number (not the standard OSC 7 "report cwd" code) so
// this signal can only come from our own hook, not from an AI CLI's own
// shell-integration output — otherwise the agent's own OSC 7 (many CLIs,
// including Claude Code, emit real OSC 7 for editor/terminal integration)
// gets misread as "the outer shell's prompt came back", falsely marking a
// still-running agent as exited after its first turn.
const FASTADE_CWD_OSC = 55123;
const SHELL_CWD_HOOK_COMMAND = ` __fastade_cwd() { printf '\\033]${FASTADE_CWD_OSC};%s\\033\\\\' "$PWD"; }; if [ -n "$ZSH_VERSION" ]; then precmd_functions+=(__fastade_cwd); else PROMPT_COMMAND="\${PROMPT_COMMAND:+$PROMPT_COMMAND; }__fastade_cwd"; fi`;
const SHELL_CWD_HOOK = `${SHELL_CWD_HOOK_COMMAND}\r`;
const AGENT_COMMANDS: Record<CliKind, string> = { codex: 'codex', claude: 'claude', gemini: 'gemini' };
const DEFAULT_WEB_AGENTS: WebAgent[] = [
  { id: 'codex', name: 'Codex', chatUrl: 'https://chatgpt.com/codex', usageUrl: 'https://chatgpt.com/codex', accent: '#8eefaa', enabled: true, builtIn: true },
  { id: 'claude', name: 'Claude', chatUrl: 'https://claude.ai/new', usageUrl: 'https://claude.ai/settings/usage', accent: '#e7a56f', enabled: true, builtIn: true },
];

function loadFontSize(): number {
  const stored = Number(localStorage.getItem(FONT_SIZE_STORAGE_KEY));
  return Number.isFinite(stored) && stored >= MIN_FONT_SIZE && stored <= MAX_FONT_SIZE ? stored : DEFAULT_FONT_SIZE;
}

export class AppViewModel {
  sessions = $state<SessionSummary[]>([]);
  savedSessions = $state<SavedSessionProfile[]>([]);
  sshHosts = $state<SshHost[]>([]);
  selectedSessionId = $state<string | null>(null);
  selectedGroupId = $state<string>(UNGROUPED_ID);
  defaultGroupName = $state(DEFAULT_GROUP_NAME);
  groupDefinitions = $state<GroupDefinition[]>([]);
  sessionGroupIds = $state<Record<string, string>>({});
  profileGroupIds = $state<Record<string, string>>({});
  sessionProfileIds = $state<Record<string, string>>({});
  pinnedSessionIds = $state<Record<string, boolean>>({});
  draftGroupName = $state('');
  drawerOpen = $state(true);
  loading = $state(false);
  error = $state<string | null>(null);
  settingsOpen = $state(false);
  webAgents = $state<WebAgent[]>(DEFAULT_WEB_AGENTS.map((agent) => ({ ...agent })));
  agentUsage = $state<Record<string, AgentUsage>>({});
  agentDefaultModels = $state<Record<CliKind, string>>({ codex: '', claude: '', gemini: '' });
  /** Whether that CLI's own MCP config currently registers fastade's
   * read-only session bridge — read from disk on load, kept in sync as the user
   * flips the Settings toggle. */
  mcpAgentEnabled = $state<Record<CliKind, boolean>>({ codex: false, claude: false, gemini: false });
  managedServers = $state<ManagedServer[]>([]);
  serverPickerOpen = $state(false);
  lastGeneratedKey = $state<{ serverId: string; publicKey: string } | null>(null);
  fontSize = $state(loadFontSize());
  runningAgents = $state<Record<string, CliKind>>({});
  agentActivity = $state<Record<string, AgentActivity>>({});
  /** Resident memory (bytes) per running session, polled at a low frequency
   * from the host — see `refreshMemoryUsage`. */
  sessionMemory = $state<Record<string, number>>({});
  /** "working"/"waiting"/"idle" reported by a CLI's own hook, keyed by session id —
   * see `activityFor`. Only present for sessions whose CLI has hooks wired
   * up; absent for the rest, which fall back to the heuristic. */
  activityOverrides = $state<Record<string, string>>({});
  remoteBrowser = $state<{ endpoint: string; sessionId: string } | null>(null);
  /** Session ids with an upload in flight, for the status line's spinner. */
  uploadingFiles = $state<Record<string, boolean>>({});
  private homeDirectory = '';
  private unsubscribeTerminal?: () => void;
  private readonly terminalSinks = new Map<string, Set<TerminalSink>>();
  private readonly terminalWrites = new Map<string, Promise<void>>();
  private readonly hookInstalledSessionIds = new Set<string>();
  /** Recent raw PTY bytes per session, used only to scan for activity cues
   * (spinners, prompts) — separate from the terminal's own scrollback. */
  private readonly activityTails = new Map<string, string>();
  private readonly activityIdleTimers = new Map<string, ReturnType<typeof setTimeout>>();
  /** First directory each shell reported after opening at `~` — the device's
   * real home path (e.g. /root), so it can be told apart from a chosen folder. */
  private readonly shellHomes = new Map<string, string>();
  private readonly shellInputBuffers = new Map<string, string>();
  private readonly contextWrites = new Map<string, Promise<void>>();
  private usageTimer?: ReturnType<typeof setInterval>;
  private memoryTimer?: ReturnType<typeof setInterval>;
  private activityOverrideTimer?: ReturnType<typeof setInterval>;

  constructor(readonly client: DesktopClient) {}

  get ungroupedProfiles(): SavedSessionProfile[] {
    // A saved profile is a reusable launch profile, not a one-session connection
    // state. Keep it available while matching sessions run so users can launch
    // more than one session from the same profile.
    return this.savedSessions.filter((profile) => !this.validCustomGroupId(this.profileGroupIds[profile.id]));
  }

  /** The folder a saved profile opens in on its device. */
  ungroupedPathFor(profile: SavedSessionProfile): string {
    return profile.devicePaths[profile.lastEndpoint] ?? this.homePathFor(profile.lastEndpoint);
  }

  isAgentRunning(sessionId: string): boolean {
    return sessionId in this.runningAgents;
  }

  isSessionPinned(sessionId: string): boolean {
    return this.pinnedSessionIds[sessionId] === true;
  }

  toggleSessionPin(sessionId: string): void {
    if (!this.sessions.some((session) => session.id === sessionId)) return;
    const next = { ...this.pinnedSessionIds };
    if (next[sessionId]) delete next[sessionId];
    else next[sessionId] = true;
    this.pinnedSessionIds = next;
    this.persistGroups();
  }

  /** `idle` whenever no agent is running in this session's shell. Otherwise
   * prefers a CLI's own hook-reported status (ground truth) over the
   * PTY-text heuristic, when that CLI has one wired up. */
  activityFor(sessionId: string): AgentActivity {
    if (!(sessionId in this.runningAgents)) return 'idle';
    const reported = this.activityOverrides[sessionId];
    const observed = this.agentActivity[sessionId];
    // A waiting hook is the strongest signal because the agent explicitly
    // needs the user. For the other states, active PTY output must beat a
    // stale idle notification left over from the previous turn; otherwise a
    // newly working agent remains green until another lifecycle hook happens
    // to replace that stored idle value.
    // The PTY waiting cue is only a fallback for sessions without hooks
    // (e.g. remote SSH): an agent's own reply can quote prompt text such as
    // "Enter to select", which must not override a hook's `idle`.
    if (reported === 'waiting' || (reported === undefined && observed === 'waiting')) return 'waiting';
    if (reported === 'working' || observed === 'working') return 'working';
    return 'idle';
  }

  /** CLIs enabled in AI agent settings, in a stable order — the only ones
   * offered when launching an agent or setting a default model. */
  get availableAgents(): CliKind[] {
    const enabledIds = new Set(this.webAgents.filter((agent) => agent.enabled).map((agent) => agent.id));
    return (Object.keys(AGENT_COMMANDS) as CliKind[]).filter((cli) => enabledIds.has(cli));
  }

  /** Model aliases each CLI documents in its own --help (not a live fetch —
   * none of the three expose a scriptable "list models" command). Shown as
   * datalist suggestions; any value can still be typed. */
  knownModelAliases(cli: CliKind): string[] {
    return cli === 'claude' ? ['fable', 'opus', 'sonnet'] : [];
  }

  /** A session sitting in the device's home has no project folder yet. */
  hasDesignatedPath(session: CliSessionSummary): boolean {
    return !this.isHomePath(session, session.projectPath);
  }

  hasUngroupedProfile(session: SessionSummary): boolean {
    return Boolean(this.profileForSession(session));
  }

  setAgentDefaultModel(cli: CliKind, model: string): void {
    this.agentDefaultModels = { ...this.agentDefaultModels, [cli]: model.trim() };
    localStorage.setItem(AGENT_MODELS_STORAGE_KEY, JSON.stringify(this.agentDefaultModels));
  }

  get sessionGroups(): SessionGroup[] {
    const groups = new Map<string, SessionGroup>();
    groups.set(UNGROUPED_ID, {
      id: UNGROUPED_ID,
      name: this.defaultGroupName,
      custom: false,
      sessions: [],
      entries: [],
    });
    for (const definition of this.groupDefinitions) {
      groups.set(definition.id, { ...definition, custom: true, sessions: [], entries: [] });
    }

    const activeProfileIds = new Set<string>();
    for (const session of this.sessions) {
      const assignedId = this.groupForSession(session.id);
      const group = groups.get(assignedId) ?? groups.get(UNGROUPED_ID);
      group?.sessions.push(session);
      const profile = this.profileForSession(session);
      if (profile && assignedId !== UNGROUPED_ID) activeProfileIds.add(profile.id);
      group?.entries.push({
        id: session.id,
        sessionId: session.id,
        profileId: profile?.id,
        title: session.title,
        endpoint: session.endpoint,
        projectPath: session.projectPath,
        status: session.status,
        cli: session.cli,
        // A stopped session keeps its last `cli` so a pinned one can resume
        // that agent, but no agent is live in it: show the plain (grey)
        // status dot instead of the agent's green "ready" state.
        activity: session.status === 'running' ? this.activityFor(session.id) : undefined,
      });
    }
    for (const profile of this.savedSessions) {
      const groupId = this.validCustomGroupId(this.profileGroupIds[profile.id]);
      if (!groupId || activeProfileIds.has(profile.id)) continue;
      groups.get(groupId)?.entries.push({
        id: `profile:${profile.id}`,
        profileId: profile.id,
        title: profile.name,
        endpoint: profile.lastEndpoint,
        projectPath: this.ungroupedPathFor(profile),
        status: 'disconnected',
      });
    }
    return [...groups.values()];
  }

  get selectedGroup(): SessionGroup | undefined {
    return this.sessionGroups.find((group) => group.id === this.selectedGroupId);
  }

  profileForSession(session: SessionSummary): SavedSessionProfile | undefined {
    const mappedId = this.sessionProfileIds[session.id];
    return this.savedSessions.find((profile) => profile.id === mappedId)
      ?? this.savedSessions.find((profile) => this.matchesProfile(session, profile));
  }

  async load(options: { auxiliaryWindow?: boolean } = {}): Promise<void> {
    this.loading = true;
    this.error = null;
    try {
      this.restoreWebAgents();
      this.restoreAgentDefaultModels();
      this.unsubscribeTerminal ??= await this.client.subscribeToTerminal((event) => this.handleTerminalEvent(event));
      const [cliSessions, savedSessions, sshHosts, managedServers, homeDirectory] = await Promise.all([
        this.client.listSessions(),
        this.client.listSavedSessions(),
        this.client.listSshHosts(),
        this.client.listManagedServers(),
        this.client.getHomeDirectory().catch(() => null),
      ]);
      this.sessions = cliSessions;
      this.restoreRunningAgentState(cliSessions);
      this.savedSessions = savedSessions;
      this.sshHosts = sshHosts;
      this.managedServers = managedServers;
      this.homeDirectory = homeDirectory ?? '';
      this.restoreGroups();
      await this.ensureGroupedProfiles();
      if (!options.auxiliaryWindow) await this.restorePinnedSessions();
      const requested = new URLSearchParams(window.location.search).get('session');
      this.selectedSessionId = requested ?? this.sessions[0]?.id ?? null;
      const selected = this.sessions.find((session) => session.id === this.selectedSessionId);
      this.selectedGroupId = selected ? this.groupForSession(selected.id) : UNGROUPED_ID;
      if (!options.auxiliaryWindow) {
        void this.refreshAgentUsage();
        this.usageTimer ??= setInterval(() => void this.refreshAgentUsage(), 5 * 60_000);
        void this.refreshMcpAgentStatus();
      }
      void this.refreshMemoryUsage();
      void this.refreshActivityOverrides();
      if (!options.auxiliaryWindow) {
        // sysinfo refreshes the host process table; a coarse interval keeps
        // the informational badge useful without repeatedly waking a busy UI.
        this.memoryTimer ??= setInterval(() => void this.refreshMemoryUsage(), 5_000);
        this.activityOverrideTimer ??= setInterval(() => void this.refreshActivityOverrides(), 1000);
      }
    } catch (error) {
      this.error = this.message(error);
    } finally {
      this.loading = false;
    }
  }

  dispose(): void {
    this.unsubscribeTerminal?.();
    this.unsubscribeTerminal = undefined;
    this.terminalSinks.clear();
    this.terminalWrites.clear();
    this.contextWrites.clear();
    for (const timer of this.activityIdleTimers.values()) clearTimeout(timer);
    this.activityIdleTimers.clear();
    this.activityTails.clear();
    if (this.usageTimer) clearInterval(this.usageTimer);
    this.usageTimer = undefined;
    if (this.memoryTimer) clearInterval(this.memoryTimer);
    this.memoryTimer = undefined;
    if (this.activityOverrideTimer) clearInterval(this.activityOverrideTimer);
    this.activityOverrideTimer = undefined;
  }

  selectSession(sessionId: string): void {
    this.selectedSessionId = sessionId;
    if (this.sessions.some((item) => item.id === sessionId)) this.selectedGroupId = this.groupForSession(sessionId);
  }

  /** Selects the next live session in the same top-to-bottom order used by
   * the session drawer, wrapping at either end. Saved profiles and stopped
   * sessions are not open terminal panes, so keyboard navigation skips them. */
  cycleSession(reverse = false): string | null {
    const sessionIds = this.sessionGroups.flatMap((group) =>
      group.sessions.filter((session) => session.status === 'running').map((session) => session.id),
    );
    if (!sessionIds.length) return null;

    const selectedIndex = sessionIds.indexOf(this.selectedSessionId ?? '');
    const nextIndex = selectedIndex < 0
      ? (reverse ? sessionIds.length - 1 : 0)
      : (selectedIndex + (reverse ? -1 : 1) + sessionIds.length) % sessionIds.length;
    const sessionId = sessionIds[nextIndex];
    this.selectSession(sessionId);
    return sessionId;
  }

  selectGroup(groupId: string): void {
    this.selectedGroupId = groupId;
    const group = this.sessionGroups.find((item) => item.id === groupId);
    if (group?.sessions.length && !group.sessions.some((session) => session.id === this.selectedSessionId)) {
      this.selectedSessionId = group.sessions[0].id;
    }
  }

  async openGroupEntry(entry: SessionGroupEntry, groupId: string): Promise<void> {
    if (entry.sessionId) {
      this.selectSession(entry.sessionId);
      const session = this.sessions.find((item) => item.id === entry.sessionId);
      if (session && session.status !== 'running') await this.reconnectSession(entry.sessionId);
      return;
    }
    const profile = this.savedSessions.find((item) => item.id === entry.profileId);
    if (profile) await this.startSaved(profile, groupId);
  }
  toggleDrawer(): void { this.drawerOpen = !this.drawerOpen; }
  toggleSettings(): void { this.settingsOpen = !this.settingsOpen; }

  setFontSize(size: number): void {
    this.fontSize = Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, size));
    localStorage.setItem(FONT_SIZE_STORAGE_KEY, String(this.fontSize));
  }
  increaseFontSize(): void { this.setFontSize(this.fontSize + 1); }
  decreaseFontSize(): void { this.setFontSize(this.fontSize - 1); }
  resetFontSize(): void { this.setFontSize(DEFAULT_FONT_SIZE); }
  reportError(error: unknown): void { this.error = this.message(error); }

  saveWebAgent(agent: WebAgent): void {
    const normalized = { ...agent, name: agent.name.trim(), chatUrl: agent.chatUrl.trim(), usageUrl: agent.usageUrl?.trim() || undefined };
    if (!normalized.name || !normalized.chatUrl) return;
    const exists = this.webAgents.some((item) => item.id === normalized.id);
    this.webAgents = exists
      ? this.webAgents.map((item) => item.id === normalized.id ? normalized : item)
      : [...this.webAgents, normalized];
    this.persistWebAgents();
    void this.refreshAgentUsage();
  }

  deleteWebAgent(id: string): void {
    this.webAgents = this.webAgents.filter((agent) => agent.id !== id);
    const next = { ...this.agentUsage };
    delete next[id];
    this.agentUsage = next;
    this.persistWebAgents();
  }

  async refreshAgentUsage(): Promise<void> {
    const enabled = this.webAgents.filter((agent) => agent.enabled);
    const entries = await Promise.all(enabled.map(async (agent): Promise<[string, AgentUsage]> => {
      try { return [agent.id, await this.client.getAgentUsage(agent.id)]; }
      catch (error) {
        return [agent.id, { agentId: agent.id, status: 'error', windows: [], message: this.message(error) }];
      }
    }));
    this.agentUsage = Object.fromEntries(entries);
  }

  private static readonly LOCAL_CLI_KINDS: CliKind[] = ['codex', 'claude'];

  async refreshMcpAgentStatus(): Promise<void> {
    const entries = await Promise.all(AppViewModel.LOCAL_CLI_KINDS.map(async (cli): Promise<[CliKind, boolean]> => {
      try { return [cli, await this.client.getMcpAgentStatus(cli)]; }
      catch { return [cli, false]; }
    }));
    this.mcpAgentEnabled = Object.fromEntries(entries) as Record<CliKind, boolean>;
  }

  async setMcpAgentEnabled(cli: CliKind, enabled: boolean): Promise<void> {
    const previous = this.mcpAgentEnabled[cli];
    this.mcpAgentEnabled = { ...this.mcpAgentEnabled, [cli]: enabled };
    try { await this.client.setMcpAgentEnabled(cli, enabled); }
    catch (error) {
      this.mcpAgentEnabled = { ...this.mcpAgentEnabled, [cli]: previous };
      this.error = this.message(error);
    }
  }

  async refreshActivityOverrides(): Promise<void> {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    if (!this.sessions.some((session) => session.status === 'running')) return;
    try {
      this.activityOverrides = await this.client.sessionActivityOverrides();
      // A CLI's own hook firing is ground truth that an agent is running —
      // even if our own typed-command detector missed the launch (a command
      // replayed from shell history, pasted outside the tracked input path,
      // or a session restored while a hook-driven agent was mid-turn). Without
      // this, `activityFor` would still short-circuit to 'idle' and ignore
      // the override entirely.
      for (const sessionId of Object.keys(this.activityOverrides)) {
        if (sessionId in this.runningAgents) continue;
        const session = this.sessions.find((item) => item.id === sessionId);
        if (session?.cli) this.runningAgents = { ...this.runningAgents, [sessionId]: session.cli };
      }
    }
    catch { /* best-effort; a failed poll just leaves the heuristic driving the badge */ }
  }

  /** Skips the host round-trip while the window is hidden — nothing is
   * showing the badge, so there is nothing to keep fresh. */
  async refreshMemoryUsage(): Promise<void> {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    if (!this.sessions.some((session) => session.status === 'running')) return;
    try { this.sessionMemory = await this.client.sessionMemoryUsage(); }
    catch { /* best-effort UI hint; a failed poll just leaves the last value shown */ }
  }

  /** e.g. "128 MB" for the session header badge, or `undefined` before the
   * first poll resolves. */
  memoryLabelFor(sessionId: string): string | undefined {
    const bytes = this.sessionMemory[sessionId];
    if (bytes === undefined) return undefined;
    return `${Math.max(1, Math.round(bytes / (1024 * 1024)))} MB`;
  }

  openServerPicker(): void {
    this.serverPickerOpen = true;
  }

  closeServerPicker(): void {
    this.serverPickerOpen = false;
    this.lastGeneratedKey = null;
  }

  async startLocalShell(): Promise<void> {
    await this.createSession(this.selectedGroupId, 'local');
  }

  async startOnSshHost(alias: string): Promise<void> {
    this.serverPickerOpen = false;
    await this.createSession(this.selectedGroupId, alias);
  }

  async startOnManagedServer(server: ManagedServer): Promise<void> {
    this.serverPickerOpen = false;
    await this.createSession(this.selectedGroupId, `managed:${server.id}`);
  }

  async createManagedServer(input: CreateServerInput): Promise<void> {
    this.error = null;
    try {
      const created = await this.client.createManagedServer(input);
      this.managedServers = [...this.managedServers, created.server];
      this.lastGeneratedKey = created.generatedPublicKey
        ? { serverId: created.server.id, publicKey: created.generatedPublicKey }
        : null;
    } catch (error) {
      this.error = this.message(error);
    }
  }

  async updateManagedServer(input: UpdateServerInput): Promise<void> {
    this.error = null;
    try {
      const updated = await this.client.updateManagedServer(input);
      this.managedServers = this.managedServers.map((server) => server.id === updated.server.id ? updated.server : server);
      this.lastGeneratedKey = updated.generatedPublicKey
        ? { serverId: updated.server.id, publicKey: updated.generatedPublicKey }
        : null;
    } catch (error) {
      this.error = this.message(error);
    }
  }

  async deleteManagedServer(id: string): Promise<void> {
    try {
      await this.client.deleteManagedServer(id);
      this.managedServers = this.managedServers.filter((server) => server.id !== id);
      if (this.lastGeneratedKey?.serverId === id) this.lastGeneratedKey = null;
    } catch (error) {
      this.error = this.message(error);
    }
  }

  dismissGeneratedKey(): void {
    this.lastGeneratedKey = null;
  }

  /** Opens a shell on the picked server. With no explicit folder it starts
   * at the server's home; the folder can be changed from the session header
   * afterwards, and that is also what names the session. */
  async createSession(targetGroupId: string, endpoint: string, projectPath?: string): Promise<void> {
    const server = endpoint.trim();
    if (!server) {
      this.error = 'Choose a server.';
      return;
    }
    const path = projectPath ?? this.homePathFor(server);
    this.error = null;
    try {
      const created = await this.client.createSession({
        title: this.groupName(server, path),
        endpoint: server,
        projectPath: path,
      });
      this.sessions = [created, ...this.sessions];
      void this.syncSessionStatuses();
      if (targetGroupId !== UNGROUPED_ID) {
        const profile = await this.ensureProfile(created);
        this.sessionGroupIds = { ...this.sessionGroupIds, [created.id]: targetGroupId };
        this.sessionProfileIds = { ...this.sessionProfileIds, [created.id]: profile.id };
        this.profileGroupIds = { ...this.profileGroupIds, [profile.id]: targetGroupId };
        this.persistGroups();
      }
      this.selectedSessionId = created.id;
      this.selectedGroupId = targetGroupId;
    } catch (error) {
      this.error = this.message(error);
    }
  }

  async toggleUngroupedProfile(session: SessionSummary): Promise<void> {
    const existing = this.profileForSession(session);
    if (existing) {
      if (this.validCustomGroupId(this.profileGroupIds[existing.id])) return;
      await this.removeUngroupedProfile(existing.id);
      return;
    }
    this.error = null;
    try {
      const saved = await this.client.saveSessionProfile({
        name: session.title,
        endpoint: session.endpoint,
        projectPath: session.projectPath,
      });
      this.savedSessions = [...this.savedSessions, saved];
      this.sessionProfileIds = { ...this.sessionProfileIds, [session.id]: saved.id };
      this.persistGroups();
    } catch (error) { this.error = this.message(error); }
  }

  /** Opens a shell for a saved profile: its device, at its saved folder. */
  async startSaved(profile: SavedSessionProfile, targetGroupId = this.selectedGroupId): Promise<void> {
    await this.createSession(targetGroupId, profile.lastEndpoint, this.ungroupedPathFor(profile));
  }

  createGroup(): void {
    const name = this.draftGroupName.trim();
    if (!name) return;
    const group = { id: `group-${crypto.randomUUID()}`, name };
    this.groupDefinitions = [...this.groupDefinitions, group];
    this.draftGroupName = '';
    this.selectedGroupId = group.id;
    this.persistGroups();
  }

  renameGroup(groupId: string, name: string): void {
    const trimmedName = name.trim();
    if (!trimmedName) return;
    if (groupId === UNGROUPED_ID) {
      this.defaultGroupName = trimmedName;
      this.persistGroups();
      return;
    }
    if (!this.groupDefinitions.some((group) => group.id === groupId)) return;
    this.groupDefinitions = this.groupDefinitions.map((group) => group.id === groupId ? { ...group, name: trimmedName } : group);
    this.persistGroups();
  }

  deleteGroup(groupId: string): void {
    if (groupId === UNGROUPED_ID) return;
    this.groupDefinitions = this.groupDefinitions.filter((group) => group.id !== groupId);
    this.sessionGroupIds = Object.fromEntries(Object.entries(this.sessionGroupIds).filter(([, assignedId]) => assignedId !== groupId));
    this.profileGroupIds = Object.fromEntries(Object.entries(this.profileGroupIds).filter(([, assignedId]) => assignedId !== groupId));
    if (this.selectedGroupId === groupId) this.selectedGroupId = UNGROUPED_ID;
    this.persistGroups();
  }

  async moveSessionToGroup(entryId: string, groupId: string): Promise<void> {
    try {
      const validGroupId = groupId === UNGROUPED_ID || this.groupDefinitions.some((group) => group.id === groupId) ? groupId : UNGROUPED_ID;
      if (entryId.startsWith('profile:')) {
        const profileId = entryId.slice('profile:'.length);
        if (!this.savedSessions.some((profile) => profile.id === profileId)) return;
        const nextProfiles = { ...this.profileGroupIds };
        if (validGroupId === UNGROUPED_ID) delete nextProfiles[profileId];
        else nextProfiles[profileId] = validGroupId;
        this.profileGroupIds = nextProfiles;
        this.selectedGroupId = validGroupId;
        this.persistGroups();
        return;
      }
      const session = this.sessions.find((item) => item.id === entryId);
      if (!session) return;
      // A saved profile can be shared by several running sessions. Snapshot
      // each session's current group before moving the profile so dragging one
      // row does not make every session for the same project jump with it.
      const currentGroupIds = new Map(this.sessions.map((item) => [item.id, this.groupForSession(item.id)]));
      const profile = this.profileForSession(session) ?? (validGroupId !== UNGROUPED_ID ? await this.ensureProfile(session) : undefined);
      const relatedSessionIds = profile
        ? this.sessions.filter((item) => this.profileForSession(item)?.id === profile.id).map((item) => item.id)
        : [entryId];
      const next = { ...this.sessionGroupIds };
      const nextSessionProfiles = { ...this.sessionProfileIds };
      for (const sessionId of relatedSessionIds) {
        next[sessionId] = sessionId === entryId
          ? validGroupId
          : currentGroupIds.get(sessionId) ?? UNGROUPED_ID;
      }
      if (profile) nextSessionProfiles[entryId] = profile.id;
      this.sessionGroupIds = next;
      this.sessionProfileIds = nextSessionProfiles;
      if (profile) {
        const nextProfiles = { ...this.profileGroupIds };
        if (validGroupId === UNGROUPED_ID) delete nextProfiles[profile.id];
        else nextProfiles[profile.id] = validGroupId;
        this.profileGroupIds = nextProfiles;
      }
      this.selectedGroupId = validGroupId;
      this.selectedSessionId = entryId;
      this.persistGroups();
    } catch (error) {
      this.error = this.message(error);
    }
  }

  /** Deletes a sidebar entry dragged onto the trash zone: closes the live
   * session it points to, or — for a not-yet-started saved profile entry —
   * deletes the saved profile outright (unlike removeUngroupedProfile, this
   * does not skip profiles that are still pinned to a custom group). */
  async deleteEntry(entryId: string): Promise<void> {
    if (entryId.startsWith('profile:')) {
      const profileId = entryId.slice('profile:'.length);
      try {
        await this.client.deleteSavedSession(profileId);
        this.savedSessions = this.savedSessions.filter((profile) => profile.id !== profileId);
        this.sessionProfileIds = Object.fromEntries(Object.entries(this.sessionProfileIds).filter(([, id]) => id !== profileId));
        this.profileGroupIds = Object.fromEntries(Object.entries(this.profileGroupIds).filter(([id]) => id !== profileId));
        this.persistGroups();
      } catch (error) { this.error = this.message(error); }
      return;
    }
    await this.closeSession(entryId);
  }

  /** The shell reported its cwd at a fresh prompt (after every `cd`). The
   * session name follows the folder as long as it is still the automatic
   * one — a manual rename sticks — and stays the device name while the shell
   * is in the device's home, so a fresh login never gets named "root".
   * A fresh prompt also means whatever agent was running has exited. */
  updateCurrentDirectory(sessionId: string, path: string): void {
    if (!path.trim()) return;
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session) return;
    if (!this.shellHomes.has(sessionId) && (session.projectPath === '~' || session.projectPath === this.homeDirectory)) {
      this.shellHomes.set(sessionId, path);
    }
    if (session.projectPath !== path) {
      this.rememberProfileLink(session);
      const follows = session.title === this.titleFor(session, session.projectPath);
      const title = follows ? this.titleFor(session, path) : session.title;
      this.sessions = this.sessions.map((item) => item.id === sessionId ? { ...item, projectPath: path, title } : item);
      void this.persistSessionContext(sessionId, title, path);
    }
    if (sessionId in this.runningAgents) this.markAgentExited(sessionId);
  }

  /** Folder button: local opens the OS folder picker, SSH opens the remote
   * browser. Picking a folder `cd`s the real shell there and renames the
   * session after it. */
  async changeFolder(sessionId: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session) return;
    if (session.endpoint !== 'local') {
      this.remoteBrowser = { endpoint: session.endpoint, sessionId };
      return;
    }
    try {
      const selected = await this.client.selectFolder(session.projectPath);
      if (selected) await this.confirmShellDirectory(sessionId, selected);
    } catch (error) {
      this.error = this.message(error);
    }
  }

  /** Upload button: picks a file via the OS file dialog and drops it at the
   * root of the session's project folder — copied locally, or uploaded over
   * SSH for a remote session's endpoint. */
  async uploadFile(sessionId: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session) return;
    try {
      const selected = await this.client.selectFile();
      if (!selected) return;
      this.uploadingFiles = { ...this.uploadingFiles, [sessionId]: true };
      await this.client.uploadFileToSession(sessionId, selected);
    } catch (error) {
      this.error = this.message(error);
    } finally {
      const { [sessionId]: _removed, ...rest } = this.uploadingFiles;
      this.uploadingFiles = rest;
    }
  }

  async confirmShellDirectory(sessionId: string, path: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session) return;
    if (sessionId in this.runningAgents) {
      this.error = 'Cannot change folder while an AI agent is running. Stop it with ■ first.';
      return;
    }
    const title = this.titleFor(session, path);
    this.rememberProfileLink(session);
    // Apply optimistically so persistSessionContext's own staleness guard
    // (which compares against the in-memory session) doesn't discard this
    // confirmed value while the shell's own OSC7 ack for the `cd` is still
    // in flight.
    this.sessions = this.sessions.map((item) => item.id === sessionId ? { ...item, projectPath: path } : item);
    if (path !== session.projectPath) await this.writeTerminal(sessionId, ` cd -- ${this.shellQuote(path)}\r`);
    await this.persistSessionContext(sessionId, title, path);
  }

  closeRemoteBrowser(): void {
    this.remoteBrowser = null;
  }

  get remoteBrowserInitialPath(): string {
    const target = this.remoteBrowser;
    if (!target) return '~';
    const session = this.sessions.find((item) => item.id === target.sessionId);
    return session?.projectPath ?? '~';
  }

  applyRemoteFolderSelection(path: string): void {
    const target = this.remoteBrowser;
    this.remoteBrowser = null;
    if (target) void this.confirmShellDirectory(target.sessionId, path);
  }

  /** Runs an AI agent inside the session's shell, with that agent's default
   * model from settings. The header then shows it until it exits. */
  async launchAgent(sessionId: string, cli: CliKind): Promise<void> {
    if (sessionId in this.runningAgents) return;
    const model = this.agentDefaultModels[cli] || undefined;
    const command = model ? `${AGENT_COMMANDS[cli]} --model ${this.shellQuote(model)}` : AGENT_COMMANDS[cli];
    // Mark first so the typed-command detector in writeTerminal doesn't
    // register the same launch a second time.
    void this.markAgentRunning(sessionId, cli, model);
    await this.writeTerminal(sessionId, `${command}\r`);
  }

  /** Reopens pinned terminals after an app restart. Codex and Claude both
   * scope their "last" conversation to the current project directory. Gemini
   * is intentionally excluded because it only resumes explicitly tagged
   * `/chat save` checkpoints rather than the last conversation automatically. */
  private async restorePinnedSessions(): Promise<void> {
    const pinned = this.sessions.filter((session) => this.isSessionPinned(session.id));
    for (const saved of pinned) {
      try {
        let restored = saved;
        if (saved.status !== 'running') {
          restored = await this.client.reconnectSession(saved.id);
          this.sessions = this.sessions.map((session) => session.id === saved.id ? restored : session);
          void this.syncSessionStatuses();
        }
        if (!restored.cli) {
          this.installShellHook(saved.id);
          continue;
        }
        if (restored.cli === 'gemini') {
          this.installShellHook(saved.id);
          const shell = await this.client.updateSessionAgent(saved.id);
          this.sessions = this.sessions.map((session) => session.id === saved.id ? shell : session);
          continue;
        }
        const model = restored.model ? ` --model ${this.shellQuote(restored.model)}` : '';
        const command = restored.cli === 'codex'
          ? `codex resume --last${model}`
          : `claude --continue${model}`;
        // Do both in one shell command. A separate hook-install command would
        // briefly return to the shell prompt and look like the resumed agent
        // had exited before it even started.
        this.hookInstalledSessionIds.add(saved.id);
        void this.markAgentRunning(saved.id, restored.cli, restored.model);
        await this.writeTerminal(saved.id, `${SHELL_CWD_HOOK_COMMAND}; clear; ${command}\r`);
      } catch (error) {
        this.error = `Could not restore ${saved.title}: ${this.message(error)}`;
      }
    }
  }

  /** ■: quit the running agent and drop back to the shell prompt. All three
   * CLIs quit on a second Ctrl-C; in a bare shell it is just a harmless ^C. */
  async exitAgent(sessionId: string): Promise<void> {
    await this.writeTerminal(sessionId, '\u0003');
    await new Promise((resolve) => setTimeout(resolve, 250));
    await this.writeTerminal(sessionId, '\u0003');
  }

  async removeUngroupedProfile(id: string): Promise<void> {
    if (this.validCustomGroupId(this.profileGroupIds[id])) return;
    try {
      await this.client.deleteSavedSession(id);
      this.savedSessions = this.savedSessions.filter((profile) => profile.id !== id);
      this.sessionProfileIds = Object.fromEntries(Object.entries(this.sessionProfileIds).filter(([, profileId]) => profileId !== id));
      this.persistGroups();
    } catch (error) { this.error = this.message(error); }
  }

  registerTerminal(sessionId: string, sink: TerminalSink): () => void {
    const sinks = this.terminalSinks.get(sessionId) ?? new Set<TerminalSink>();
    sinks.add(sink);
    this.terminalSinks.set(sessionId, sinks);
    void this.client.terminalSnapshot(sessionId).then((snapshot) => {
      if (sinks.has(sink) && snapshot) {
        // Reclassify an already-running agent after an app reload. Live PTY
        // events emitted while no window was attached are only in this snapshot.
        this.observeAgentOutput(sessionId, snapshot);
        // A raw transcript can contain terminal capability queries emitted by
        // an earlier CLI. Mark it as replay so xterm's generated replies are
        // rendered but are not sent into the currently running PTY as input.
        sink(snapshot, true);
      }
    }).catch((error) => { this.error = this.message(error); });
    const session = this.sessions.find((item) => item.id === sessionId);
    if (session?.status === 'running') this.installShellHook(sessionId);
    return () => {
      sinks.delete(sink);
      if (!sinks.size) this.terminalSinks.delete(sessionId);
    };
  }

  async writeTerminal(sessionId: string, data: string): Promise<void> {
    if (this.sessions.find((session) => session.id === sessionId)?.status !== 'running') return;
    // The clearest start-of-turn signal is the user's submit itself. This is
    // especially useful for Codex, whose full-screen TUI can redraw the idle
    // composer before its first busy frame arrives.
    if (sessionId in this.runningAgents && /[\r\n]/.test(data)) {
      this.activityTails.delete(sessionId);
      this.clearActivityOverride(sessionId);
      this.setAgentActivity(sessionId, 'working');
      this.armIdleTimer(sessionId);
    }
    // Browser input and the WebKit Hangul adapter can emit adjacent chunks in
    // the same event turn. Serialize IPC writes so committed text always reaches
    // the PTY before the key (for example Enter) that follows it.
    const actions = this.trackShellInput(sessionId, data);
    const previous = this.terminalWrites.get(sessionId) ?? Promise.resolve();
    const write = previous.catch(() => undefined).then(() => this.client.writeTerminal(sessionId, data));
    this.terminalWrites.set(sessionId, write);
    try {
      await write;
    } catch (error) {
      this.error = this.message(error);
    } finally {
      if (this.terminalWrites.get(sessionId) === write) this.terminalWrites.delete(sessionId);
    }
    if (actions.launchedAgent && !(sessionId in this.runningAgents)) {
      void this.markAgentRunning(sessionId, actions.launchedAgent);
    }
  }

  async resizeTerminal(sessionId: string, cols: number, rows: number): Promise<void> {
    if (this.sessions.find((session) => session.id === sessionId)?.status !== 'running') return;
    try { await this.client.resizeTerminal(sessionId, cols, rows); }
    catch (error) { this.error = this.message(error); }
  }

  async interruptSession(sessionId = this.selectedSessionId): Promise<void> {
    if (!sessionId) return;
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session || session.status !== 'running') return;
    try { await this.client.interruptSession(sessionId); }
    catch (error) { this.error = this.message(error); }
  }

  async reconnectSession(sessionId: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    if (!session) return;
    this.error = null;
    try {
      const reconnected = await this.client.reconnectSession(sessionId);
      this.sessions = this.sessions.map((session) => session.id === sessionId ? reconnected : session);
      void this.syncSessionStatuses();
      this.installShellHook(sessionId);
      this.selectSession(sessionId);
    } catch (error) {
      this.error = this.message(error);
      void this.syncSessionStatuses();
    }
  }

  /** Reconciles session rows with the backend, which owns every PTY. A
   * terminal that exits immediately (e.g. an unreachable SSH host) emits its
   * exit event before the create/reconnect call returns; that event finds no
   * row (or is overwritten by the call's `running` result), which would leave
   * a dead session showing green. Sessions the backend no longer knows are
   * dropped for the same reason. */
  async syncSessionStatuses(): Promise<void> {
    try {
      const backend = new Map((await this.client.listSessions()).map((session) => [session.id, session]));
      const stale = this.sessions.filter((session) => !backend.has(session.id)).map((session) => session.id);
      const changed = this.sessions.some((session) => backend.get(session.id)?.status !== session.status);
      if (!stale.length && !changed) return;
      this.sessions = this.sessions.flatMap((session) => {
        const current = backend.get(session.id);
        if (!current) return [];
        if (current.status === session.status) return [session];
        if (current.status !== 'running') this.clearAgentActivity(session.id);
        return [{ ...session, status: current.status }];
      });
      for (const sessionId of stale) {
        this.hookInstalledSessionIds.delete(sessionId);
        this.clearAgentActivity(sessionId);
      }
      if (this.selectedSessionId && !backend.has(this.selectedSessionId)) this.selectedSessionId = this.sessions[0]?.id ?? null;
    } catch {
      // Best effort: the next exit event or sync corrects the rows.
    }
  }

  async closeSession(sessionId: string): Promise<void> {
    try {
      const session = this.sessions.find((item) => item.id === sessionId);
      if (!session) return;
      await this.client.closeSession(sessionId);
      this.sessions = this.sessions.filter((session) => session.id !== sessionId);
      this.hookInstalledSessionIds.delete(sessionId);
      this.shellHomes.delete(sessionId);
      this.shellInputBuffers.delete(sessionId);
      this.clearAgentActivity(sessionId);
      if (this.pinnedSessionIds[sessionId]) {
        const next = { ...this.pinnedSessionIds };
        delete next[sessionId];
        this.pinnedSessionIds = next;
        this.persistGroups();
      }
      if (sessionId in this.runningAgents) {
        const next = { ...this.runningAgents };
        delete next[sessionId];
        this.runningAgents = next;
      }
      if (this.sessionGroupIds[sessionId]) {
        const next = { ...this.sessionGroupIds };
        delete next[sessionId];
        this.sessionGroupIds = next;
        this.persistGroups();
      }
      if (this.sessionProfileIds[sessionId]) {
        const next = { ...this.sessionProfileIds };
        delete next[sessionId];
        this.sessionProfileIds = next;
        this.persistGroups();
      }
      const selectedGroup = this.sessionGroups.find((group) => group.id === this.selectedGroupId);
      if (!selectedGroup) this.selectedGroupId = UNGROUPED_ID;
      if (this.selectedSessionId === sessionId) {
        this.selectedSessionId = this.selectedGroup?.sessions[0]?.id ?? this.sessions[0]?.id ?? null;
      }
    } catch (error) { this.error = this.message(error); }
  }

  async popOut(sessionId: string): Promise<void> {
    this.selectedSessionId = sessionId;
    try { await this.client.openSessionWindow(sessionId); }
    catch (error) { this.error = this.message(error); }
  }

  async renameSession(sessionId: string, title: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    const nextTitle = title.trim();
    if (!session || !nextTitle) return;
    try {
      this.sessions = this.sessions.map((item) => item.id === sessionId ? { ...item, title: nextTitle } : item);
      await this.persistSessionContext(sessionId, nextTitle, session.projectPath);
    } catch (error) { this.error = this.message(error); }
  }

  private handleTerminalEvent(event: TerminalEvent): void {
    if (event.kind === 'output') {
      this.terminalSinks.get(event.sessionId)?.forEach((sink) => sink(event.content));
      this.observeAgentOutput(event.sessionId, event.content);
      return;
    }
    if (event.kind === 'activity') {
      if (event.content !== 'working' && event.content !== 'waiting' && event.content !== 'idle') return;
      this.activityOverrides = { ...this.activityOverrides, [event.sessionId]: event.content };
      if (!(event.sessionId in this.runningAgents)) {
        const session = this.sessions.find((item) => item.id === event.sessionId);
        if (session?.cli) this.runningAgents = { ...this.runningAgents, [event.sessionId]: session.cli };
      }
      return;
    }
    const session = this.sessions.find((item) => item.id === event.sessionId);
    if (!session) return;
    this.hookInstalledSessionIds.delete(event.sessionId);
    const status: CliSessionSummary['status'] = event.content === 'completed' ? 'completed' : 'failed';
    this.sessions = this.sessions.map((item) => item.id === event.sessionId ? { ...item, status } : item);
    this.clearAgentActivity(event.sessionId);
  }

  /** Heuristic-only: classifies what a running agent is doing from its raw
   * PTY bytes. Never treated as ground truth — just a hint for the UI badge. */
  private observeAgentOutput(sessionId: string, chunk: string): void {
    if (!(sessionId in this.runningAgents)) return;
    const printable = printableTerminalText(chunk);
    // Device-status replies, cursor moves, title changes, etc. are terminal
    // housekeeping, not evidence that the agent is still generating.
    if (!/\S/.test(printable)) return;
    const tail = ((this.activityTails.get(sessionId) ?? '') + printable).slice(-ACTIVITY_TAIL_CHARS);
    this.activityTails.set(sessionId, tail);

    const cli = this.runningAgents[sessionId];
    const waitingIndex = lastPatternIndex(tail, WAITING_PATTERN);
    const idleIndex = cli === 'codex' ? lastPatternIndex(tail, CODEX_IDLE_PATTERN) : -1;
    const busyIndex = lastPatternIndex(tail, BUSY_PATTERN);
    // Full-screen TUIs repaint old and new states into the same PTY chunk.
    // Whichever explicit cue was painted last represents the current screen.
    if (waitingIndex > busyIndex) {
      this.setAgentActivity(sessionId, 'waiting');
      this.clearIdleTimer(sessionId);
      return;
    }
    if (idleIndex > busyIndex) {
      this.setAgentActivity(sessionId, 'idle');
      this.clearIdleTimer(sessionId);
      return;
    }
    this.setAgentActivity(sessionId, 'working');
    this.armIdleTimer(sessionId);
  }

  /** No output at all for AGENT_IDLE_TIMEOUT_MS while an agent is running
   * almost always means it finished its response and returned to its idle
   * composer, even when the prompt has no distinctive text. */
  private armIdleTimer(sessionId: string): void {
    this.clearIdleTimer(sessionId);
    const timer = setTimeout(() => {
      this.activityIdleTimers.delete(sessionId);
      if (sessionId in this.runningAgents) this.setAgentActivity(sessionId, 'idle');
    }, AGENT_IDLE_TIMEOUT_MS);
    this.activityIdleTimers.set(sessionId, timer);
  }

  private clearIdleTimer(sessionId: string): void {
    const timer = this.activityIdleTimers.get(sessionId);
    if (timer) clearTimeout(timer);
    this.activityIdleTimers.delete(sessionId);
  }

  private setAgentActivity(sessionId: string, activity: AgentActivity): void {
    if (this.agentActivity[sessionId] === activity) return;
    this.agentActivity = { ...this.agentActivity, [sessionId]: activity };
  }

  private clearAgentActivity(sessionId: string): void {
    this.clearIdleTimer(sessionId);
    this.activityTails.delete(sessionId);
    if (!(sessionId in this.agentActivity)) return;
    const next = { ...this.agentActivity };
    delete next[sessionId];
    this.agentActivity = next;
  }

  private clearActivityOverride(sessionId: string): void {
    if (sessionId in this.activityOverrides) {
      const next = { ...this.activityOverrides };
      delete next[sessionId];
      this.activityOverrides = next;
    }
    // Clear the backend snapshot too, otherwise the next fallback poll would
    // briefly restore the stale waiting state after the user answered.
    void this.client.clearSessionActivityOverride(sessionId).catch(() => undefined);
  }

  private restoreRunningAgentState(sessions: CliSessionSummary[]): void {
    for (const timer of this.activityIdleTimers.values()) clearTimeout(timer);
    this.activityIdleTimers.clear();
    this.activityTails.clear();

    const running = sessions.filter(
      (session): session is CliSessionSummary & { cli: CliKind } => session.status === 'running' && Boolean(session.cli),
    );
    this.runningAgents = Object.fromEntries(running.map((session) => [session.id, session.cli]));
    this.agentActivity = Object.fromEntries(running.map((session) => [session.id, 'working' as const]));
    for (const session of running) this.armIdleTimer(session.id);
  }

  private matchesProfile(session: CliSessionSummary, profile: SavedSessionProfile): boolean {
    return profile.devicePaths[session.endpoint] === session.projectPath;
  }

  private installShellHook(sessionId: string): void {
    if (this.hookInstalledSessionIds.has(sessionId)) return;
    this.hookInstalledSessionIds.add(sessionId);
    void this.writeTerminal(sessionId, SHELL_CWD_HOOK);
    void this.writeTerminal(sessionId, 'clear\r');
  }

  private groupForSession(sessionId: string): string {
    const assignedId = this.sessionGroupIds[sessionId];
    // `ungrouped` is an explicit override when a session has a profile that
    // belongs to a custom group.
    if (assignedId === UNGROUPED_ID) return UNGROUPED_ID;
    if (this.validCustomGroupId(assignedId)) return assignedId;
    const session = this.sessions.find((item) => item.id === sessionId);
    const profile = session ? this.profileForSession(session) : undefined;
    return this.validCustomGroupId(profile ? this.profileGroupIds[profile.id] : undefined) ?? UNGROUPED_ID;
  }

  private validCustomGroupId(groupId: string | undefined): string | undefined {
    return groupId && this.groupDefinitions.some((group) => group.id === groupId) ? groupId : undefined;
  }

  private async ensureProfile(session: SessionSummary): Promise<SavedSessionProfile> {
    const existing = this.profileForSession(session);
    if (existing) return existing;
    const saved = await this.client.saveSessionProfile({
      name: session.title,
      endpoint: session.endpoint,
      projectPath: session.projectPath,
    });
    this.savedSessions = [...this.savedSessions, saved];
    return saved;
  }

  private rememberProfileLink(session: SessionSummary): void {
    const profile = this.profileForSession(session);
    if (!profile || this.sessionProfileIds[session.id] === profile.id) return;
    this.sessionProfileIds = { ...this.sessionProfileIds, [session.id]: profile.id };
    this.persistGroups();
  }

  private async ensureGroupedProfiles(): Promise<void> {
    let changed = false;
    for (const session of this.sessions) {
      const groupId = this.validCustomGroupId(this.sessionGroupIds[session.id]);
      if (!groupId) continue;
      const profile = await this.ensureProfile(session);
      if (this.sessionProfileIds[session.id] !== profile.id) {
        this.sessionProfileIds = { ...this.sessionProfileIds, [session.id]: profile.id };
        changed = true;
      }
      if (this.profileGroupIds[profile.id] !== groupId) {
        this.profileGroupIds = { ...this.profileGroupIds, [profile.id]: groupId };
        changed = true;
      }
    }
    if (changed) this.persistGroups();
  }

  /** Session name for a folder: its last segment, or the server's name
   * while the shell is still sitting in the server's home. */
  private groupName(endpoint: string, projectPath: string): string {
    if (projectPath === '~' || (endpoint === 'local' && projectPath === this.homeDirectory)) return this.endpointLabel(endpoint);
    return projectPath.split(/[\\/]/).filter(Boolean).at(-1) ?? projectPath;
  }

  private titleFor(session: CliSessionSummary, path: string): string {
    return this.isHomePath(session, path) ? this.endpointLabel(session.endpoint) : this.groupName(session.endpoint, path);
  }

  /** Human-readable label for an endpoint: `local`, an SSH alias, or a
   * managed server's saved name instead of its raw `managed:<id>` form. */
  endpointLabel(endpoint: string): string {
    const id = endpoint.startsWith('managed:') ? endpoint.slice('managed:'.length) : null;
    if (!id) return endpoint;
    return this.managedServers.find((server) => server.id === id)?.name ?? endpoint;
  }

  private isHomePath(session: CliSessionSummary, path: string): boolean {
    return path === '~'
      || (session.endpoint === 'local' && path === this.homeDirectory)
      || this.shellHomes.get(session.id) === path;
  }

  private restoreGroups(): void {
    try {
      const stored = JSON.parse(localStorage.getItem(GROUPS_STORAGE_KEY) ?? '{}') as { defaultName?: string; groups?: GroupDefinition[]; membership?: Record<string, string>; profileMembership?: Record<string, string>; sessionProfiles?: Record<string, string>; pinnedSessions?: Record<string, boolean> };
      this.defaultGroupName = typeof stored.defaultName === 'string' && stored.defaultName.trim() ? stored.defaultName.trim() : DEFAULT_GROUP_NAME;
      this.groupDefinitions = Array.isArray(stored.groups) ? stored.groups.filter((group) => group?.id && group?.name) : [];
      this.sessionGroupIds = stored.membership && typeof stored.membership === 'object' ? stored.membership : {};
      this.profileGroupIds = stored.profileMembership && typeof stored.profileMembership === 'object' ? stored.profileMembership : {};
      this.sessionProfileIds = stored.sessionProfiles && typeof stored.sessionProfiles === 'object' ? stored.sessionProfiles : {};
      this.pinnedSessionIds = stored.pinnedSessions && typeof stored.pinnedSessions === 'object' ? stored.pinnedSessions : {};
    } catch {
      this.defaultGroupName = DEFAULT_GROUP_NAME;
      this.groupDefinitions = [];
      this.sessionGroupIds = {};
      this.profileGroupIds = {};
      this.sessionProfileIds = {};
      this.pinnedSessionIds = {};
    }
  }

  private persistGroups(): void {
    localStorage.setItem(GROUPS_STORAGE_KEY, JSON.stringify({ defaultName: this.defaultGroupName, groups: this.groupDefinitions, membership: this.sessionGroupIds, profileMembership: this.profileGroupIds, sessionProfiles: this.sessionProfileIds, pinnedSessions: this.pinnedSessionIds }));
  }

  private restoreWebAgents(): void {
    try {
      const stored = JSON.parse(localStorage.getItem(WEB_AGENTS_STORAGE_KEY) ?? 'null') as WebAgent[] | null;
      if (Array.isArray(stored)) this.webAgents = stored.filter((agent) => agent?.id && agent.id !== 'gemini' && agent?.name && agent?.chatUrl);
    } catch { this.webAgents = DEFAULT_WEB_AGENTS.map((agent) => ({ ...agent })); }
  }

  private restoreAgentDefaultModels(): void {
    try {
      const stored = JSON.parse(localStorage.getItem(AGENT_MODELS_STORAGE_KEY) ?? '{}') as Partial<Record<CliKind, string>>;
      this.agentDefaultModels = {
        codex: stored.codex ?? '',
        claude: stored.claude ?? '',
        gemini: stored.gemini ?? '',
      };
    } catch {
      this.agentDefaultModels = { codex: '', claude: '', gemini: '' };
    }
  }

  private persistWebAgents(): void {
    localStorage.setItem(WEB_AGENTS_STORAGE_KEY, JSON.stringify(this.webAgents));
  }

  private async persistSessionContext(sessionId: string, title: string, projectPath: string): Promise<void> {
    const session = this.sessions.find((item) => item.id === sessionId);
    const profileId = session ? this.profileForSession(session)?.id : undefined;
    const previous = this.contextWrites.get(sessionId) ?? Promise.resolve();
    const write = previous.catch(() => undefined).then(async () => {
      const updated = await this.client.updateSessionContext(sessionId, title, projectPath);
      if (profileId) {
        const profile = await this.client.updateSavedSession(profileId, title, updated.endpoint, projectPath);
        this.savedSessions = this.savedSessions.map((item) => item.id === profile.id ? profile : item);
      }
      const current = this.sessions.find((session) => session.id === sessionId);
      if (current?.projectPath === projectPath) {
        this.sessions = this.sessions.map((session) => session.id === sessionId ? updated : session);
      }
    });
    this.contextWrites.set(sessionId, write);
    try { await write; }
    catch (error) { this.error = this.message(error); }
    finally { if (this.contextWrites.get(sessionId) === write) this.contextWrites.delete(sessionId); }
  }

  private trackShellInput(sessionId: string, data: string): ShellInputActions {
    const actions: ShellInputActions = { launchedAgent: null };
    if (data === SHELL_CWD_HOOK || sessionId in this.runningAgents) return actions;
    let buffer = this.shellInputBuffers.get(sessionId) ?? '';
    for (const character of data) {
      if (character === '\r' || character === '\n') {
        const command = this.cleanShellCommand(buffer);
        const launched = command.match(/^(?:command\s+)?(codex|claude|gemini)(?:\s|$)/);
        if (launched) actions.launchedAgent = launched[1] as CliKind;
        buffer = '';
      } else if (character === '\x7f' || character === '\b') {
        buffer = Array.from(buffer).slice(0, -1).join('');
      } else if (character === '\x15') {
        buffer = '';
      } else if (character >= ' ') {
        buffer += character;
      }
    }
    this.shellInputBuffers.set(sessionId, buffer);
    return actions;
  }

  private async markAgentRunning(sessionId: string, cli: CliKind, model?: string): Promise<void> {
    this.runningAgents = { ...this.runningAgents, [sessionId]: cli };
    this.shellInputBuffers.delete(sessionId);
    this.activityTails.delete(sessionId);
    this.setAgentActivity(sessionId, 'working');
    this.armIdleTimer(sessionId);
    try {
      const updated = await this.client.updateSessionAgent(sessionId, cli, model);
      this.sessions = this.sessions.map((item) => item.id === sessionId ? { ...item, cli: updated.cli, model: updated.model } : item);
    } catch (error) {
      this.error = this.message(error);
    }
  }

  private markAgentExited(sessionId: string): void {
    const next = { ...this.runningAgents };
    delete next[sessionId];
    this.runningAgents = next;
    this.clearAgentActivity(sessionId);
    // A hook's last-reported value would otherwise sit in the backend store
    // forever and later look like fresh evidence that the agent is running.
    this.clearActivityOverride(sessionId);
    void this.client.updateSessionAgent(sessionId).then((updated) => {
      this.sessions = this.sessions.map((item) => item.id === sessionId ? { ...item, cli: updated.cli, model: updated.model } : item);
    }).catch(() => undefined);
  }

  private cleanShellCommand(command: string): string {
    // xterm wraps pasted input when the remote shell enables bracketed-paste
    // mode. The escape byte is filtered while buffering printable input, so
    // accept both the full sequence and its printable remainder.
    return command.replace(/(?:\x1b)?\[(?:200|201)~/g, '').trim();
  }

  /** Where a fresh shell on `endpoint` starts: the real home path locally
   * (the PTY needs an absolute cwd), `~` remotely (the login shell resolves it). */
  private homePathFor(endpoint: string): string {
    return endpoint === 'local' ? (this.homeDirectory || '~') : '~';
  }

  private shellQuote(value: string): string {
    return `'${value.replace(/'/g, `'"'"'`)}'`;
  }

  private message(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }
}
