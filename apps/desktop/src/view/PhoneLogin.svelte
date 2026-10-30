<script lang="ts">
  import { onMount } from 'svelte';
  import type { DesktopClient } from '../application/desktop-client';
  import type { PhoneKey, PhoneQr } from '../domain/tunnel';
  import ServerQr from './ServerQr.svelte';

  let { client }: { client: DesktopClient } = $props();
  let qr = $state<PhoneQr | null>(null);
  let keys = $state<PhoneKey[]>([]);
  let name = $state('');
  let busy = $state(false);
  let error = $state('');
  let open = $state(false);

  const message = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause));
  // Labels are stored as `aissh-phone-<name>-<8 hex>`; show only the name.
  const displayName = (label: string) => label.replace(/^aissh-phone-/, '').replace(/-[0-9a-f]{8}$/, '') || label;
  const refresh = () => client.listPhoneKeys().then((list) => { keys = list; }).catch((cause) => { error = message(cause); });

  async function create(): Promise<void> {
    if (busy) return;
    busy = true;
    error = '';
    try { qr = await client.createPhoneQr(name); }
    catch (cause) { error = message(cause); }
    finally { busy = false; void refresh(); }
  }

  async function revoke(key: PhoneKey): Promise<void> {
    error = '';
    try { await client.revokePhoneKey(key.id); }
    catch (cause) { error = message(cause); }
    await refresh();
  }

  onMount(() => { void refresh(); return () => { qr = null; }; });
</script>

<section class="phone-login">
  <button type="button" class="toggle" aria-expanded={open} onclick={() => { open = !open; }}>
    <span class="chevron" class:open aria-hidden="true">▸</span>
    <strong>Phone login (aissh)</strong>
    {#if keys.length}<span class="count">{keys.length}</span>{/if}
  </button>
  {#if open}
    <div class="heading">
      <small>Creates a key for your phone and shows a one-time QR. With the desktop tunnel connected the phone signs in through the backend and works from anywhere; otherwise it uses this Mac's Wi-Fi address (same network only).</small>
      <input class="name" bind:value={name} maxlength="40" placeholder="Phone name (e.g. Galaxy S24)" aria-label="Phone name" disabled={busy || qr !== null} />
      <button type="button" class="create" disabled={busy || qr !== null} onclick={() => void create()}>{busy ? 'Creating…' : 'Show QR'}</button>
    </div>
    {#if qr}
      {@const phone = qr}
      <ServerQr server={{ name: phone.name, host: phone.host, port: phone.port, username: phone.username, jump: phone.jump }}
        secretKind="key" autoSecret ttlSeconds={60}
        loadCredentials={async () => ({ key: phone.key })} onClose={() => { qr = null; }} />
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if keys.length}
      <ul>
        {#each keys as key (key.id)}
          <li><span>{displayName(key.label)}</span><button type="button" onclick={() => void revoke(key)}>Revoke</button></li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  .phone-login { display: grid; gap: 8px; padding: 10px 16px; border-bottom: 1px solid #29352e; }
  .heading { display: flex; align-items: center; justify-content: space-between; gap: 14px; }
  strong { font-size: 11px; }
  small { display: block; color: #748078; font-size: 9px; }
  .toggle { display: flex; align-items: center; gap: 6px; width: 100%; padding: 0; border: none; text-align: left; }
  .chevron { display: inline-block; color: #748078; font-size: 9px; transition: transform .15s; }
  .chevron.open { transform: rotate(90deg); }
  .count { padding: 0 5px; border-radius: 8px; background: #29352e; color: #a9b6ad; font-size: 9px; }
  .error { margin: 0; color: #ffb4aa; font-size: 9px; overflow-wrap: anywhere; }
  button { flex: 0 0 auto; padding: 5px 9px; border: 1px solid #34443a; border-radius: 6px; background: transparent; color: inherit; font-size: 10px; cursor: pointer; }
  button:disabled { opacity: .45; cursor: default; }
  .name { flex: 0 1 170px; min-width: 0; padding: 5px 8px; border: 1px solid #34443a; border-radius: 6px; background: transparent; color: inherit; font-size: 10px; }
  ul { display: grid; gap: 4px; margin: 0; padding: 0; list-style: none; }
  li { display: flex; align-items: center; justify-content: space-between; gap: 8px; font-size: 10px; }
  li span { min-width: 0; overflow-wrap: anywhere; }
</style>
