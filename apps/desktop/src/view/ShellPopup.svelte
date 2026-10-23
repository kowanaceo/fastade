<script lang="ts">
  import { onMount } from 'svelte';
  import { createDesktopClient } from '../application/client-provider';
  import type { CliSessionSummary, SessionStatus } from '../domain/session';
  import TerminalPane from './TerminalPane.svelte';

  export let source: CliSessionSummary;
  export let endpointLabel: string;
  export let fontSize: number;
  export let layoutRevision: number;
  export let onClose: () => void;
  export let onFocus: () => void;

  const client = createDesktopClient();
  let session: CliSessionSummary | null = null;
  let sessionId = '';
  let status: SessionStatus = 'running';
  let error = '';
  let closing = false;
  let disposed = false;
  let outputSink: ((data: string, replay?: boolean) => void) | undefined;
  let writes = Promise.resolve();
  let pendingSize: { cols: number; rows: number } | null = null;
  let resizing = false;

  async function close(): Promise<void> {
    if (closing) return;
    closing = true;
    try {
      if (sessionId) await client.closeSession(sessionId);
      sessionId = '';
      onClose();
    } catch (cause) {
      error = String(cause);
      closing = false;
    }
  }

  function registerOutput(id: string, sink: (data: string, replay?: boolean) => void): () => void {
    outputSink = sink;
    void client.terminalSnapshot(id).then((snapshot) => {
      if (outputSink === sink && snapshot) sink(snapshot, true);
    }).catch((cause) => { if (!disposed) error = String(cause); });
    return () => { if (outputSink === sink) outputSink = undefined; };
  }

  function write(id: string, data: string): void {
    if (closing || status !== 'running') return;
    writes = writes.then(() => client.writeTerminal(id, data)).catch((cause) => {
      if (!disposed) error = String(cause);
    });
  }

  async function resize(id: string, cols: number, rows: number): Promise<void> {
    if (closing || status !== 'running') return;
    pendingSize = { cols, rows };
    if (resizing) return;
    resizing = true;
    try {
      while (pendingSize && !closing && !disposed) {
        const next = pendingSize;
        pendingSize = null;
        await client.resizeTerminal(id, next.cols, next.rows);
      }
    } catch (cause) {
      if (!disposed) error = String(cause);
    } finally {
      resizing = false;
    }
  }

  onMount(() => {
    let unsubscribe: (() => void) | undefined;
    // The shell can exit (e.g. SSH failure) before createSession resolves.
    const earlyStatuses = new Map<string, SessionStatus>();
    void (async () => {
      try {
        const removeListener = await client.subscribeToTerminal((event) => {
          if (event.kind === 'status') {
            const next = event.content === 'completed' ? 'completed' : 'failed';
            if (!sessionId) earlyStatuses.set(event.sessionId, next);
            else if (event.sessionId === sessionId) status = next;
          }
          if (event.sessionId === sessionId && event.kind === 'output') outputSink?.(event.content);
        });
        if (disposed) { removeListener(); return; }
        unsubscribe = removeListener;
        const created = await client.createSession({
          title: `${source.title} — shell`,
          endpoint: source.endpoint,
          projectPath: source.projectPath,
          temporary: true,
        });
        sessionId = created.id;
        if (disposed) {
          await client.closeSession(created.id);
          sessionId = '';
          return;
        }
        status = earlyStatuses.get(created.id) ?? created.status;
        earlyStatuses.clear();
        session = created;
      } catch (cause) {
        if (!disposed) error = String(cause);
      }
    })();
    return () => {
      disposed = true;
      unsubscribe?.();
      outputSink = undefined;
      if (sessionId) void client.closeSession(sessionId).catch(() => {});
    };
  });
</script>

<div class="shell-popup" data-shell-popup role="dialog" aria-label={`Shell for ${source.title}`} tabindex="-1" oncontextmenu={(event) => event.stopPropagation()}>
  <header>
    <div><strong>Shell · {source.title}</strong><small>{endpointLabel} · {source.projectPath}</small></div>
    <button type="button" aria-label="Close shell" title="Close shell" disabled={closing} onclick={() => void close()}>×</button>
  </header>
  {#if error}<div class="popup-error" role="alert">{error}</div>{/if}
  <div class="popup-terminal">
    {#if session}
      <TerminalPane
        sessionId={session.id}
        {fontSize}
        {layoutRevision}
        {registerOutput}
        onInput={write}
        onResize={(id, cols, rows) => void resize(id, cols, rows)}
        onInterrupt={(id) => write(id, '\x1b')}
        onFocus={() => onFocus()}
        onCurrentDirectory={() => {}}
        onActivityScreen={() => {}}
      />
    {:else if !error}<div class="popup-message" role="status">Opening shell…</div>{/if}
  </div>
  {#if session && status !== 'running'}<div class="popup-message" role="status">{status === 'completed' ? 'Shell exited.' : 'Shell disconnected.'} Close this popup to return to your session.</div>{/if}
</div>

<style>
  .shell-popup { position: absolute; z-index: 3; top: 44px; right: 16px; display: flex; flex-direction: column; width: min(760px, calc(100% - 32px)); height: min(460px, calc(72% - 16px)); padding: 0; border: 1px solid #52755b; border-radius: 9px; background: #0b100d; color: #d6e0d9; box-shadow: 0 12px 36px #000a; overflow: hidden; }
  header { display: flex; flex-shrink: 0; align-items: center; justify-content: space-between; gap: 12px; padding: 7px 10px; border-bottom: 1px solid #26352b; background: #142018; font-size: 11px; }
  header div { min-width: 0; }
  strong, small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  small { margin-top: 2px; color: #94a69b; font-size: 9px; }
  button { flex-shrink: 0; padding: 0 7px; border: 1px solid #303d35; border-radius: 5px; background: #151d18; font-size: 18px; }
  button:hover { border-color: #77b989; }
  .popup-terminal { flex: 1; min-height: 0; padding: 8px 8px 0; }
  .popup-message, .popup-error { padding: 12px 16px; font-size: 12px; }
  .popup-error { color: #ffb4aa; }
</style>
