import type { ManagedServer } from './session.ts';

export const SERVER_QR_SCHEME = 'aissh://add-server';

/** A host the phone signs in to first; `host`/`port` of the connection are then
 * reached from it. The same key authenticates both hops. */
export interface ServerQrJump {
  host: string;
  port: number;
  user: string;
}

export type ServerQrConnection = Pick<ManagedServer, 'name' | 'host' | 'port' | 'username'> & {
  jump?: ServerQrJump | null;
};

export interface SshHostQr extends ServerQrConnection, ServerQrCredentials {}

export interface ServerQrCredentials {
  password?: string | null;
  /** OpenSSH private key, base64url encoded. */
  key?: string | null;
}

/** A link the aissh phone app can scan to prefill its "add server" form. The
 * parameter names (`host`, `user`) are what the app's parser requires. Secrets
 * are included only when the caller passes them explicitly. */
export function serverQrPayload(
  server: ServerQrConnection,
  credentials: ServerQrCredentials = {},
): string {
  const query = new URLSearchParams({
    host: server.host,
    user: server.username,
    port: String(server.port),
    name: server.name,
  });
  if (server.jump) {
    query.set('jump_host', server.jump.host);
    query.set('jump_port', String(server.jump.port));
    query.set('jump_user', server.jump.user);
  }
  if (credentials.password) query.set('pass', credentials.password);
  if (credentials.key) query.set('key', credentials.key);
  // URLSearchParams encodes spaces as '+', which some parsers keep literally.
  return `${SERVER_QR_SCHEME}?${query.toString().replace(/\+/g, '%20')}`;
}
