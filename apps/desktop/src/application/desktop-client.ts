import type { AgentAccount, AgentModelOption, AgentUsage, AuthStatus, AuthUser, CliKind, CliSessionSummary, CreateServerInput, CreatedServer, ManagedServer, SavedSessionProfile, SyncChangesResponse, SyncPushChange, SyncPushResponse, SyncSnapshot, UpdateServerInput, UsageSnapshot } from '../domain/session';

export interface CreateSessionInput {
  title: string;
  endpoint: string;
  projectPath: string;
}

export interface SaveSessionInput {
  name: string;
  endpoint: string;
  projectPath: string;
}

export interface SshHost {
  alias: string;
}

export interface RemoteDirectoryListing {
  path: string;
  entries: string[];
}

export interface TerminalEvent {
  sessionId: string;
  kind: 'output' | 'status' | 'activity';
  content: string;
}

export interface DesktopClient {
  listSessions(): Promise<CliSessionSummary[]>;
  listSavedSessions(): Promise<SavedSessionProfile[]>;
  saveSessionProfile(input: SaveSessionInput): Promise<SavedSessionProfile>;
  rememberSavedSessionPath(id: string, endpoint: string, projectPath: string): Promise<SavedSessionProfile>;
  updateSavedSession(id: string, name: string, endpoint: string, projectPath: string): Promise<SavedSessionProfile>;
  deleteSavedSession(id: string): Promise<void>;
  replaceSavedSessions(profiles: SavedSessionProfile[]): Promise<void>;
  listSshHosts(): Promise<SshHost[]>;
  listManagedServers(): Promise<ManagedServer[]>;
  createManagedServer(input: CreateServerInput): Promise<CreatedServer>;
  updateManagedServer(input: UpdateServerInput): Promise<CreatedServer>;
  deleteManagedServer(id: string): Promise<void>;
  listRemoteDirectory(endpoint: string, path: string): Promise<RemoteDirectoryListing>;
  getHomeDirectory(): Promise<string | null>;
  selectFolder(defaultPath?: string): Promise<string | null>;
  selectFile(defaultPath?: string): Promise<string | null>;
  selectFiles(defaultPath?: string): Promise<string[]>;
  uploadFileToSession(sessionId: string, sourcePath: string): Promise<string>;
  subscribeToTerminal(handler: (event: TerminalEvent) => void): Promise<() => void>;
  /** Notes or tasks changed outside the window, e.g. through the MCP bridge. */
  subscribeToRecordChanges(handler: () => void): Promise<() => void>;
  createSession(input: CreateSessionInput): Promise<CliSessionSummary>;
  writeTerminal(sessionId: string, data: string): Promise<void>;
  resizeTerminal(sessionId: string, cols: number, rows: number): Promise<void>;
  terminalSnapshot(sessionId: string): Promise<string>;
  updateSessionContext(sessionId: string, title: string, projectPath: string): Promise<CliSessionSummary>;
  updateSessionAgent(sessionId: string, cli?: CliKind, model?: string): Promise<CliSessionSummary>;
  refreshSessionAgentMetadata(sessionId: string): Promise<CliSessionSummary>;
  listAgentModels(endpoint: string, cli: CliKind): Promise<AgentModelOption[]>;
  interruptSession(sessionId: string): Promise<CliSessionSummary>;
  reconnectSession(sessionId: string): Promise<CliSessionSummary>;
  closeSession(sessionId: string): Promise<void>;
  openSessionWindow(sessionId: string): Promise<void>;
  /** `force` skips the short-lived cache of Claude's slow-to-read limits. */
  getAgentUsage(agentId: string, force?: boolean): Promise<AgentUsage>;
  /** Codex limits of every SSH host with a live session. `force` asks the host
   * for live limits instead of relying only on its latest session log. */
  getRemoteUsage(force?: boolean): Promise<UsageSnapshot[]>;
  getLocalAccounts(): Promise<AgentAccount[]>;
  putUsageSnapshot(snapshot: UsageSnapshot): Promise<void>;
  listUsageSnapshots(): Promise<unknown>;
  sessionMemoryUsage(): Promise<Record<string, number>>;
  sessionActivityOverrides(): Promise<Record<string, string>>;
  clearSessionActivityOverride(sessionId: string): Promise<void>;
  getMcpAgentStatus(cli: CliKind): Promise<boolean>;
  setMcpAgentEnabled(cli: CliKind, enabled: boolean): Promise<void>;
  googleAuthStatus(): Promise<AuthStatus>;
  googleSignIn(): Promise<AuthUser>;
  googleSignOut(): Promise<void>;
  syncSnapshot(): Promise<SyncSnapshot>;
  syncChanges(cursor: number, limit?: number, waitSeconds?: number): Promise<SyncChangesResponse>;
  syncPush(changes: SyncPushChange[]): Promise<SyncPushResponse>;
}
