export type CliKind = 'codex' | 'gemini' | 'claude';
export type SessionStatus = 'running' | 'completed' | 'failed';

export interface CliSessionSummary {
  kind?: 'cli';
  id: string;
  title: string;
  /** Agent currently running inside the session's shell, if any. */
  cli?: CliKind;
  model?: string;
  endpoint: string;
  projectPath: string;
  status: SessionStatus;
}

export type SessionSummary = CliSessionSummary;

export type AuthMethod =
  | { kind: 'password' }
  | { kind: 'keyFile'; path: string };

export interface ManagedServer {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth: AuthMethod;
}

export type CreateAuthInput =
  | { kind: 'password'; password: string }
  | { kind: 'keyFile'; path: string }
  | { kind: 'generateKey' };

export interface CreateServerInput {
  name: string;
  host: string;
  port?: number;
  username: string;
  auth: CreateAuthInput;
}

export type UpdateAuthInput = CreateAuthInput | { kind: 'unchanged' };

export interface UpdateServerInput {
  id: string;
  name: string;
  host: string;
  port?: number;
  username: string;
  auth: UpdateAuthInput;
}

export interface CreatedServer {
  server: ManagedServer;
  generatedPublicKey?: string;
}

export interface SavedSessionProfile {
  id: string;
  name: string;
  lastEndpoint: string;
  devicePaths: Record<string, string>;
}

export interface WebAgent {
  id: string;
  name: string;
  chatUrl: string;
  usageUrl?: string;
  accent: string;
  enabled: boolean;
  builtIn?: boolean;
}

export interface UsageWindow {
  label: string;
  remainingPercent: number;
  resetsAt?: number;
  resetText?: string;
}

export interface AgentUsage {
  agentId: string;
  status: 'available' | 'unavailable' | 'error';
  windows: UsageWindow[];
  message?: string;
}
