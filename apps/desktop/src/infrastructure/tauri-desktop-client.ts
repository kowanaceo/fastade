import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type { CreateSessionInput, DesktopClient, RemoteDirectoryListing, SaveSessionInput, SshHost, TerminalEvent } from '../application/desktop-client';
import type { AgentUsage, CliKind, CliSessionSummary, CreateServerInput, CreatedServer, ManagedServer, SavedSessionProfile, UpdateServerInput } from '../domain/session';

export class TauriDesktopClient implements DesktopClient {
  listSessions(): Promise<CliSessionSummary[]> {
    return invoke<CliSessionSummary[]>('list_sessions');
  }

  listSavedSessions(): Promise<SavedSessionProfile[]> {
    return invoke<SavedSessionProfile[]>('list_saved_sessions');
  }

  saveSessionProfile(input: SaveSessionInput): Promise<SavedSessionProfile> {
    return invoke<SavedSessionProfile>('save_session_profile', { input });
  }

  rememberSavedSessionPath(id: string, endpoint: string, projectPath: string): Promise<SavedSessionProfile> {
    return invoke<SavedSessionProfile>('remember_saved_session_path', { id, endpoint, projectPath });
  }

  updateSavedSession(id: string, name: string, endpoint: string, projectPath: string): Promise<SavedSessionProfile> {
    return invoke<SavedSessionProfile>('update_saved_session', { id, name, endpoint, projectPath });
  }

  deleteSavedSession(id: string): Promise<void> {
    return invoke<void>('delete_saved_session', { id });
  }

  listSshHosts(): Promise<SshHost[]> {
    return invoke<SshHost[]>('list_ssh_hosts');
  }

  listRemoteDirectory(endpoint: string, path: string): Promise<RemoteDirectoryListing> {
    return invoke<RemoteDirectoryListing>('list_remote_directory', { endpoint, path });
  }

  listManagedServers(): Promise<ManagedServer[]> {
    return invoke<ManagedServer[]>('list_managed_servers');
  }

  createManagedServer(input: CreateServerInput): Promise<CreatedServer> {
    return invoke<CreatedServer>('create_managed_server', { input });
  }

  updateManagedServer(input: UpdateServerInput): Promise<CreatedServer> {
    return invoke<CreatedServer>('update_managed_server', { input });
  }

  deleteManagedServer(id: string): Promise<void> {
    return invoke<void>('delete_managed_server', { id });
  }

  getHomeDirectory(): Promise<string | null> {
    return invoke<string>('get_home_directory').catch(() => null);
  }

  async selectFolder(defaultPath?: string): Promise<string | null> {
    const selected = await open({ directory: true, multiple: false, defaultPath });
    return typeof selected === 'string' ? selected : null;
  }

  async selectFile(defaultPath?: string): Promise<string | null> {
    const selected = await open({ directory: false, multiple: false, defaultPath });
    return typeof selected === 'string' ? selected : null;
  }

  uploadFileToSession(sessionId: string, sourcePath: string): Promise<string> {
    return invoke<string>('upload_file_to_session', { sessionId, sourcePath });
  }

  async subscribeToTerminal(handler: (event: TerminalEvent) => void): Promise<() => void> {
    return listen<TerminalEvent>('terminal-event', (event) => handler(event.payload));
  }

  createSession(input: CreateSessionInput): Promise<CliSessionSummary> {
    return invoke<CliSessionSummary>('create_session', { input });
  }

  writeTerminal(sessionId: string, data: string): Promise<void> {
    return invoke<void>('write_terminal', { sessionId, data });
  }

  resizeTerminal(sessionId: string, cols: number, rows: number): Promise<void> {
    return invoke<void>('resize_terminal', { sessionId, cols, rows });
  }

  terminalSnapshot(sessionId: string): Promise<string> {
    return invoke<string>('terminal_snapshot', { sessionId });
  }

  updateSessionContext(sessionId: string, title: string, projectPath: string): Promise<CliSessionSummary> {
    return invoke<CliSessionSummary>('update_session_context', { sessionId, title, projectPath });
  }

  updateSessionAgent(sessionId: string, cli?: CliKind, model?: string): Promise<CliSessionSummary> {
    return invoke<CliSessionSummary>('update_session_agent', { sessionId, cli: cli ?? null, model: model ?? null });
  }

  interruptSession(sessionId: string): Promise<CliSessionSummary> {
    return invoke<CliSessionSummary>('interrupt_session', { sessionId });
  }

  reconnectSession(sessionId: string): Promise<CliSessionSummary> {
    return invoke<CliSessionSummary>('reconnect_session', { sessionId });
  }

  closeSession(sessionId: string): Promise<void> {
    return invoke<void>('close_session', { sessionId });
  }

  openSessionWindow(sessionId: string): Promise<void> {
    return invoke<void>('open_session_window', { sessionId });
  }

  getAgentUsage(agentId: string): Promise<AgentUsage> {
    return invoke<AgentUsage>('get_agent_usage', { agentId });
  }

  sessionMemoryUsage(): Promise<Record<string, number>> {
    return invoke<Record<string, number>>('session_memory_usage');
  }

  sessionActivityOverrides(): Promise<Record<string, string>> {
    return invoke<Record<string, string>>('session_activity_overrides');
  }

  clearSessionActivityOverride(sessionId: string): Promise<void> {
    return invoke<void>('clear_session_activity_override', { sessionId });
  }

  getMcpAgentStatus(cli: CliKind): Promise<boolean> {
    return invoke<boolean>('mcp_agent_status', { cli });
  }

  setMcpAgentEnabled(cli: CliKind, enabled: boolean): Promise<void> {
    return invoke<void>('set_mcp_agent_enabled', { cli, enabled });
  }

}
