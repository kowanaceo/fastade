export interface TunnelSettings {
  backendHost: string;
  sshPort: number | null;
  identityFile: string;
}

export type TunnelPhase = 'off' | 'connecting' | 'connected' | 'reconnecting' | 'failed' | 'stopping';

export interface TunnelStatus {
  supported: boolean;
  enabled: boolean;
  phase: TunnelPhase;
  settings: TunnelSettings;
  port: number | null;
  error: string | null;
  syncError: string | null;
  retryInSeconds: number | null;
}

export interface PhoneQr {
  name: string;
  /** Set while the tunnel is connected: the phone signs in here first. */
  jump: { host: string; port: number; user: string } | null;
  host: string;
  port: number;
  username: string;
  /** OpenSSH private key PEM, base64url encoded. Exists only in memory. */
  key: string;
}

export interface PhoneKey {
  id: string;
  label: string;
}
