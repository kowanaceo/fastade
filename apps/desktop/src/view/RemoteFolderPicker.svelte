<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import type { DesktopClient } from '../application/desktop-client';

  let { endpoint, label, initialPath, client, onSelect, onClose }: {
    endpoint: string;
    /** Human-readable name to display instead of the raw endpoint id. */
    label: string;
    initialPath: string;
    client: DesktopClient;
    onSelect: (path: string) => void;
    onClose: () => void;
  } = $props();

  // Seeded once from the prop; navigating updates it independently afterward.
  let currentPath = $state(untrack(() => initialPath));
  let entries = $state<string[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  async function load(path: string): Promise<void> {
    loading = true;
    error = null;
    try {
      const result = await client.listRemoteDirectory(endpoint, path);
      currentPath = result.path;
      entries = result.entries;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }

  function parentOf(path: string): string | null {
    if (path === '/' || !path) return null;
    const trimmed = path.replace(/\/$/, '');
    const index = trimmed.lastIndexOf('/');
    if (index <= 0) return '/';
    return trimmed.slice(0, index);
  }

  function navigate(name: string): void {
    void load(`${currentPath.replace(/\/$/, '')}/${name}`);
  }

  function goUp(): void {
    const parent = parentOf(currentPath);
    if (parent) void load(parent);
  }

  onMount(() => { void load(currentPath); });
</script>

<div class="folder-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
  <div class="folder-panel" role="dialog" aria-modal="true" aria-label={`Browse folders on ${label}`}>
    <header>
      <div><strong>{label}</strong><small title={currentPath}>{currentPath}</small></div>
      <button class="icon-button" aria-label="Close" onclick={onClose}>×</button>
    </header>
    <div class="folder-list" aria-busy={loading}>
      {#if loading}
        <p class="folder-state">Loading…</p>
      {:else if error}
        <p class="folder-state folder-error">{error}</p>
      {:else}
        {#if parentOf(currentPath)}
          <button class="folder-row folder-up" onclick={goUp}>.. (parent folder)</button>
        {/if}
        {#if !entries.length}
          <p class="folder-state">No subfolders.</p>
        {/if}
        {#each entries as name (name)}
          <button class="folder-row" onclick={() => navigate(name)}>{name}</button>
        {/each}
      {/if}
    </div>
    <div class="folder-actions">
      <button onclick={onClose}>Cancel</button>
      <button class="primary" disabled={loading || Boolean(error)} onclick={() => onSelect(currentPath)}>Choose this folder</button>
    </div>
  </div>
</div>
