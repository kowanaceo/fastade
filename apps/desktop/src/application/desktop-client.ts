import type { AgentUsage, CliKind, CliSessionSummary, CreateServerInput, CreatedServer, ManagedServer, SavedSessionProfile, UpdateServerInput } from '../domain/session';

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
  listSshHosts(): Promise<SshHost[]>;
  listManagedServers(): Promise<ManagedServer[]>;
  createManagedServer(input: CreateServerInput): Promise<CreatedServer>;
  updateManagedServer(input: UpdateServerInput): Promise<CreatedServer>;
  deleteManagedServer(id: string): Promise<void>;
  listRemoteDirectory(endpoint: string, path: string): Promise<RemoteDirectoryListing>;
  getHomeDirectory(): Promise<string | null>;
  selectFolder(defaultPath?: string): Promise<string | null>;
  selectFile(defaultPath?: string): Promise<string | null>;
  uploadFileToSession(sessionId: string, sourcePath: string): Promise<string>;
  subscribeToTerminal(handler: (event: TerminalEvent) => void): Promise<() => void>;
  createSession(input: CreateSessionInput): Promise<CliSessionSummary>;
  writeTerminal(sessionId: string, data: string): Promise<void>;
  resizeTerminal(sessionId: string, cols: number, rows: number): Promise<void>;
  terminalSnapshot(sessionId: string): Promise<string>;
  updateSessionContext(sessionId: string, title: string, projectPath: string): Promise<CliSessionSummary>;
  updateSessionAgent(sessionId: string, cli?: CliKind, model?: string): Promise<CliSessionSummary>;
  interruptSession(sessionId: string): Promise<CliSessionSummary>;
  reconnectSession(sessionId: string): Promise<CliSessionSummary>;
  closeSession(sessionId: string): Promise<void>;
  openSessionWindow(sessionId: string): Promise<void>;
  getAgentUsage(agentId: string): Promise<AgentUsage>;
  sessionMemoryUsage(): Promise<Record<string, number>>;
  sessionActivityOverrides(): Promise<Record<string, string>>;
  clearSessionActivityOverride(sessionId: string): Promise<void>;
  getMcpAgentStatus(cli: CliKind): Promise<boolean>;
  setMcpAgentEnabled(cli: CliKind, enabled: boolean): Promise<void>;
}
