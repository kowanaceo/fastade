<script lang="ts">
  import { onMount } from 'svelte';
  import type { DesktopClient } from '../application/desktop-client';
  import type { TunnelStatus } from '../domain/tunnel';
  import { tunnelMessages } from './tunnel-messages';

  let { client, authenticated }: { client: DesktopClient; authenticated: boolean } = $props();
  const messages = tunnelMessages();
  let status = $state<TunnelStatus | null>(null);
  let busy = $state(false);
  let error = $state('');

  const errorMessage = (code: unknown) => messages.errors[String(code) as keyof typeof messages.errors] ?? messages.errors.fallback;

  async function toggle(): Promise<void> {
    if (!status || busy) return;
    busy = true;
    error = '';
    try { status = await client.setTunnelEnabled(!status.enabled); }
    catch (cause) { error = errorMessage(cause); }
    finally { busy = false; }
  }

  onMount(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    client.subscribeToTunnelChanges((next) => { status = next; })
      .then((stop) => { if (disposed) stop(); else unsubscribe = stop; })
      .then(() => client.tunnelStatus())
      .then((loaded) => { if (!disposed) status ??= loaded; })
      .catch((cause) => { if (!disposed) error = errorMessage(cause); });
    return () => { disposed = true; unsubscribe?.(); };
  });
</script>

{#if status?.supported}
  <section class="tunnel-settings">
    <div class="heading">
      <div>
        <strong>{messages.title}</strong>
        <small class:connected={status.phase === 'connected'} role="status">{messages.phases[status.phase]}</small>
      </div>
      <button type="button" role="switch" aria-label={messages.title} aria-checked={status.enabled}
        class:active={status.enabled} disabled={busy || status.phase === 'stopping' || (!status.enabled && !authenticated)}
        onclick={() => void toggle()}><span class="track"><span></span></span></button>
    </div>
    {#if error || status.error}<p class="error" role="alert">{error || errorMessage(status.error)}</p>{/if}
  </section>
{/if}

<style>
  .tunnel-settings { padding: 10px 16px; border-bottom: 1px solid #29352e; }
  .heading { display: flex; align-items: center; justify-content: space-between; gap: 14px; }
  strong { font-size: 11px; }
  small { display: block; margin-top: 3px; color: #748078; font-size: 9px; }
  .connected { color: #8eefaa; }
  .error { margin: 6px 0 0; color: #ffb4aa; font-size: 9px; overflow-wrap: anywhere; }
  button { background: transparent; border: none; padding: 8px; color: inherit; cursor: pointer; }
  button:disabled { opacity: .45; cursor: default; }
  .track { display: block; width: 32px; height: 18px; padding: 2px; box-sizing: border-box; border-radius: 12px; background: #405147; }
  .track span { display: block; width: 14px; height: 14px; border-radius: 50%; background: #d6e0d9; transition: transform .15s; }
  button.active .track { background: #477f57; }
  button.active .track span { transform: translateX(14px); background: #8eefaa; }
</style>
