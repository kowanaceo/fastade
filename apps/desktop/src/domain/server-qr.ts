import type { ManagedServer } from './session.ts';

export const SERVER_QR_SCHEME = 'aissh://add-server';

export interface ServerQrCredentials {
  password?: string | null;
  /** OpenSSH private key, base64url encoded. */
  key?: string | null;
}

/** A link the aissh phone app can scan to prefill its "add server" form. The
 * parameter names (`host`, `user`) are what the app's parser requires. Secrets
 * are included only when the caller passes them explicitly. */
export function serverQrPayload(
  server: Pick<ManagedServer, 'name' | 'host' | 'port' | 'username'>,
  credentials: ServerQrCredentials = {},
): string {
  const query = new URLSearchParams({
    host: server.host,
    user: server.username,
    port: String(server.port),
    name: server.name,
  });
  if (credentials.password) query.set('pass', credentials.password);
  if (credentials.key) query.set('key', credentials.key);
  // URLSearchParams encodes spaces as '+', which some parsers keep literally.
  return `${SERVER_QR_SCHEME}?${query.toString().replace(/\+/g, '%20')}`;
}
