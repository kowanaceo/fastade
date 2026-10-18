<script lang="ts">
  import qrcode from 'qrcode-generator';
  import { serverQrPayload, type ServerQrConnection, type ServerQrCredentials } from '../domain/server-qr';

  let { server, secretKind, loadCredentials, onClose }: {
    server: ServerQrConnection;
    secretKind: 'password' | 'key';
    loadCredentials: () => Promise<ServerQrCredentials>;
    onClose: () => void;
  } = $props();
  let includeSecret = $state(false);
  let credentials = $state<ServerQrCredentials>({});
  let error = $state('');
  let loading = $state(false);
  const link = $derived(serverQrPayload(server, includeSecret ? credentials : {}));
  const path = $derived.by(() => {
    const code = qrcode(0, 'L');
    code.addData(link);
    try {
      code.make();
    } catch {
      return { d: '', size: 0 };
    }
    const size = code.getModuleCount();
    let d = '';
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) if (code.isDark(y, x)) d += `M${x} ${y}h1v1h-1z`;
    }
    return { d, size };
  });

  // Secrets are read only after the user opts in.
  async function toggleSecret(checked: boolean): Promise<void> {
    error = '';
    if (!checked) { includeSecret = false; credentials = {}; return; }
    loading = true;
    try {
      credentials = await loadCredentials();
      includeSecret = true;
    } catch (cause) {
      includeSecret = false;
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }
</script>

<div class="qr-card" role="dialog" aria-label={`QR code for ${server.name}`}>
  <div class="qr-head"><strong>{server.name}</strong><button class="icon-button" aria-label="Close QR code" onclick={onClose}>×</button></div>
  {#if path.size}
  <svg class="qr-image" viewBox={`-2 -2 ${path.size + 4} ${path.size + 4}`} role="img" aria-label="Server QR code" shape-rendering="crispEdges">
    <rect x="-2" y="-2" width={path.size + 4} height={path.size + 4} fill="#fff" />
    <path d={path.d} fill="#000" />
  </svg>
  {:else}<small class="qr-error">Too much data for a QR code. Turn off the password/key option.</small>{/if}
  <label class="qr-secret"><input type="checkbox" checked={includeSecret || loading} disabled={loading} onchange={(event) => void toggleSecret(event.currentTarget.checked)} /> {loading ? 'Loading…' : `Include ${secretKind === 'password' ? 'password' : 'private key'}`}</label>
  {#if error}<small class="qr-error">{error}</small>{/if}
  <small>{server.username}@{server.host}:{server.port}</small>
  <small class="qr-note">{includeSecret ? 'This QR contains your credentials. Scan it privately and close it afterwards.' : 'Scan with the aissh app. Credentials are not included; enter them in the app.'}</small>
</div>

<style>
  .qr-card { display: grid; flex: 0 0 auto; min-width: 0; justify-items: center; gap: 10px; padding: 14px; border: 1px solid #34443a; border-radius: 8px; background: #0d120f; }
  .qr-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; width: 100%; color: #dce5df; font-size: 11px; }
  .qr-image { width: min(220px, 100%); height: auto; aspect-ratio: 1; border-radius: 4px; }
  small { max-width: 100%; overflow-wrap: anywhere; color: #87948b; font-size: 9px; text-align: center; }
  .qr-head strong { min-width: 0; overflow-wrap: anywhere; }
  .qr-secret { display: flex; align-items: center; gap: 5px; color: #b8c4bc; font-size: 10px; }
  .qr-error { color: #e2919a; }
  .qr-note { color: #6d7971; }
</style>
