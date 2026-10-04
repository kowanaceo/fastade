<script lang="ts">
  import { onMount } from 'svelte';
  import type { SessionRecordRepository } from '../application/session-record-repository';
  import { TASK_STATUSES, validateRecordDraft, type SessionRecord, type TaskStatus } from '../domain/session-record';

  // Mount a new instance (keyed by profileId) when switching saved sessions.
  let { profileId, sessionName, repository, onClose }: {
    profileId: string;
    sessionName: string;
    repository: SessionRecordRepository;
    onClose: () => void;
  } = $props();
  let records = $state<SessionRecord[]>([]);
  let loading = $state(true);
  let busy = $state(false);
  let error = $state('');
  let kind = $state<'note' | 'task'>('note');
  let editing = $state<SessionRecord | null>(null);
  let editorOpen = $state(false);
  let title = $state('');
  let content = $state('');
  let status = $state<TaskStatus>('todo');
  let deletingId = $state<string | null>(null);
  let disposed = false;
  const filtered = $derived(records.filter((record) => record.kind === kind));

  function message(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
  }

  async function refresh(): Promise<void> {
    loading = true;
    error = '';
    try {
      const loaded = await repository.list(profileId);
      if (!disposed) records = loaded;
    } catch (cause) {
      if (!disposed) error = message(cause);
    } finally {
      if (!disposed) loading = false;
    }
  }

  onMount(() => {
    void refresh();
    return () => { disposed = true; };
  });

  function edit(record: SessionRecord | null): void {
    editing = record;
    title = record?.title ?? '';
    content = record?.content ?? '';
    status = record?.kind === 'task' ? record.status : 'todo';
    editorOpen = true;
    error = '';
  }

  async function save(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (busy) return;
    busy = true;
    error = '';
    try {
      const draft = validateRecordDraft(kind === 'task'
        ? { kind, title, content, status }
        : { kind, title, content });
      const saved = editing
        ? await repository.update(editing, draft)
        : await repository.create(profileId, draft);
      if (disposed) return;
      records = editing ? records.map((record) => record.id === saved.id ? saved : record) : [...records, saved];
      editorOpen = false;
      editing = null;
    } catch (cause) {
      // Keep the user's draft when authentication, validation or a conflict fails.
      if (!disposed) error = message(cause);
    } finally {
      if (!disposed) busy = false;
    }
  }

  async function remove(record: SessionRecord): Promise<void> {
    if (busy) return;
    busy = true;
    error = '';
    try {
      await repository.delete(record);
      if (!disposed) {
        records = records.filter((item) => item.id !== record.id);
        deletingId = null;
      }
    } catch (cause) {
      if (!disposed) error = message(cause);
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section class="records-panel" aria-label={`Notes and tasks for ${sessionName}`}>
  <header>
    <div><strong>{sessionName}</strong><small>Notes and tasks</small></div>
    <button type="button" disabled={busy || editorOpen} onclick={onClose}>Close</button>
  </header>
  <nav aria-label="Record type">
    <button type="button" class:active={kind === 'note'} aria-pressed={kind === 'note'} disabled={busy || editorOpen} onclick={() => { kind = 'note'; deletingId = null; }}>Notes</button>
    <button type="button" class:active={kind === 'task'} aria-pressed={kind === 'task'} disabled={busy || editorOpen} onclick={() => { kind = 'task'; deletingId = null; }}>Tasks</button>
    <button type="button" disabled={busy || loading || editorOpen} onclick={() => edit(null)}>+ {kind === 'note' ? 'Note' : 'Task'}</button>
    <button type="button" disabled={busy || loading || editorOpen} onclick={() => void refresh()}>Refresh</button>
  </nav>
  {#if error}<p class="record-error" role="alert">{error}</p>{/if}
  {#if editorOpen}
    <form onsubmit={save}>
      <label>Title<input bind:value={title} required disabled={busy} /></label>
      <label>Content<textarea bind:value={content} rows="8" disabled={busy}></textarea></label>
      {#if kind === 'task'}
        <label>Status<select bind:value={status} disabled={busy}>
          {#each TASK_STATUSES as value}<option value={value}>{value}</option>{/each}
        </select></label>
      {/if}
      <div class="record-actions">
        <button type="button" disabled={busy} onclick={() => { editorOpen = false; editing = null; error = ''; }}>Cancel</button>
        <button type="submit" disabled={busy || !title.trim()}>{busy ? 'Saving…' : 'Save'}</button>
      </div>
    </form>
  {:else if loading}<p role="status">Loading…</p>
  {:else}
    {#each filtered as record (record.id)}
      <article>
        <div class="record-heading"><strong>{record.title}</strong>{#if record.kind === 'task'}<span class="record-status" data-status={record.status}>{record.status}</span>{/if}</div>
        <p class="record-content">{record.content}</p>
        <div class="record-actions">
          {#if deletingId === record.id}
            <span>Delete this {record.kind}?</span>
            <button type="button" disabled={busy} onclick={() => { deletingId = null; }}>Cancel</button>
            <button type="button" disabled={busy} onclick={() => void remove(record)}>Delete</button>
          {:else}
            <button type="button" disabled={busy} onclick={() => edit(record)}>Edit</button>
            <button type="button" disabled={busy} onclick={() => { deletingId = record.id; }}>Delete</button>
          {/if}
        </div>
      </article>
    {:else}{#if !error}<p>No {kind === 'note' ? 'notes' : 'tasks'} yet.</p>{/if}{/each}
  {/if}
</section>

<style>
  .records-panel { display: flex; flex-direction: column; gap: 16px; padding: 20px; min-width: 0; color: #e5e7eb; background: #171a20; }
  header, nav, .record-heading, .record-actions { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
  header { justify-content: space-between; }
  header div { display: grid; gap: 4px; }
  small { color: #a1a8b5; }
  button, input, textarea, select { font: inherit; color: inherit; background: #232833; border: 1px solid #414958; border-radius: 6px; padding: 8px 10px; }
  button { cursor: pointer; }
  button:disabled { opacity: .5; cursor: default; }
  button.active { border-color: #8eefaa; color: #8eefaa; }
  form, label { display: grid; gap: 8px; }
  form { gap: 16px; }
  input, textarea { width: 100%; box-sizing: border-box; }
  textarea { resize: vertical; }
  article { padding: 14px; border: 1px solid #414958; border-radius: 8px; }
  .record-heading strong { overflow-wrap: anywhere; }
  .record-content { white-space: pre-wrap; overflow-wrap: anywhere; }
  .record-status { font-size: 12px; padding: 3px 8px; border-radius: 10px; background: #343b49; }
  .record-status[data-status='in progress'] { color: #ffd780; }
  .record-status[data-status='done'] { color: #8eefaa; }
  .record-error { color: #ff9b9b; }
  .record-actions { justify-content: flex-end; }
</style>
