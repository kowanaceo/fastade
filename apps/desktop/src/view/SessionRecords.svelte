<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import type { SessionRecordRepository } from '../application/session-record-repository';
  import { TASK_STATUSES, validateRecordDraft, type SessionRecord, type TaskStatus } from '../domain/session-record';

  // Mount a new instance (keyed by profileId) when switching saved sessions.
  let { profileId, sessionName, repository, revision = 0, syncNote = '', onChanged = () => {}, onClose }: {
    profileId: string;
    sessionName: string;
    repository: SessionRecordRepository;
    /** Changes when a sync replaced the local records, so the list reloads. */
    revision?: number;
    syncNote?: string;
    onChanged?: () => void;
    onClose: () => void;
  } = $props();
  let seenRevision = untrack(() => revision);
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
  const notes = $derived(records.filter((record) => record.kind === 'note'));
  const tasks = $derived(records.filter((record) => record.kind === 'task'));

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

  // Reload after a sync, but never while an edit is open or saving.
  $effect(() => {
    if (revision === seenRevision || editorOpen || busy) return;
    seenRevision = revision;
    void refresh();
  });

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

  function add(nextKind: 'note' | 'task'): void {
    kind = nextKind;
    deletingId = null;
    edit(null);
  }

  function statusLabel(value: TaskStatus): string {
    if (value === 'todo') return 'To do';
    if (value === 'in progress') return 'In progress';
    return 'Done';
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
      onChanged();
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
        onChanged();
      }
    } catch (cause) {
      if (!disposed) error = message(cause);
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section class="records-panel" aria-label={`Notes and tasks for ${sessionName}`}>
  <header class="panel-header">
    <div class="panel-title">
      <span class="project-mark" aria-hidden="true"></span>
      <div><strong>{sessionName}</strong><small>Notes &amp; tasks{syncNote ? ` · ${syncNote}` : ''}</small></div>
    </div>
    <div class="panel-actions">
      <button class="icon-button" type="button" title="Refresh" aria-label="Refresh notes and tasks" disabled={busy || loading || editorOpen} onclick={() => void refresh()}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2.34 5.66M20 5v6h-6" /></svg>
      </button>
      <button class="icon-button close-button" type="button" title="Close" aria-label="Close notes and tasks" disabled={busy || editorOpen} onclick={onClose}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>
      </button>
    </div>
  </header>
  {#if error}<p class="record-error" role="alert">{error}</p>{/if}
  {#if editorOpen}
    <form class="record-editor" onsubmit={save}>
      <div class="editor-heading">
        <div><span class="editor-kicker">{editing ? 'Edit' : 'New'} {kind}</span><strong>{editing ? editing.title : kind === 'note' ? 'Capture a note' : 'Create a task'}</strong></div>
        <span class:task-accent={kind === 'task'} class="kind-badge">{kind === 'note' ? 'Note' : 'Task'}</span>
      </div>
      <label>Title<input bind:value={title} placeholder={kind === 'note' ? 'Note title' : 'What needs to be done?'} required disabled={busy} /></label>
      <label>Details<textarea bind:value={content} rows="7" placeholder="Add details…" disabled={busy}></textarea></label>
      {#if kind === 'task'}
        <label>Status<select bind:value={status} disabled={busy}>
          {#each TASK_STATUSES as value}<option value={value}>{statusLabel(value)}</option>{/each}
        </select></label>
      {/if}
      <div class="record-actions">
        <button class="secondary-button" type="button" disabled={busy} onclick={() => { editorOpen = false; editing = null; error = ''; }}>Cancel</button>
        <button class="primary-button" type="submit" disabled={busy || !title.trim()}>{busy ? 'Saving…' : editing ? 'Save changes' : `Add ${kind}`}</button>
      </div>
    </form>
  {:else if loading}<div class="loading-state" role="status"><span></span>Loading notes and tasks…</div>
  {:else}
    <div class="record-columns">
      <section class="record-column note-column" aria-labelledby="notes-heading">
        <div class="column-header">
          <div><span class="column-icon note-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M6 3h9l3 3v15H6zM9 11h6M9 15h4M15 3v4h4" /></svg></span><strong id="notes-heading">Notes</strong><span class="record-count">{notes.length}</span></div>
          <button class="add-button" type="button" aria-label="Add note" title="Add note" disabled={busy} onclick={() => add('note')}><span aria-hidden="true">+</span></button>
        </div>
        <div class="record-list">
          {#each notes as record (record.id)}
            <article>
              <div class="record-heading"><strong>{record.title}</strong></div>
              {#if record.content}<p class="record-content">{record.content}</p>{/if}
              <div class="record-actions">
                {#if deletingId === record.id}
                  <span class="delete-prompt">Delete this note?</span>
                  <button class="text-button" type="button" disabled={busy} onclick={() => { deletingId = null; }}>Cancel</button>
                  <button class="text-button danger" type="button" disabled={busy} onclick={() => void remove(record)}>Delete</button>
                {:else}
                  <button class="text-button" type="button" disabled={busy} onclick={() => edit(record)}>Edit</button>
                  <button class="text-button danger" type="button" disabled={busy} onclick={() => { deletingId = record.id; }}>Delete</button>
                {/if}
              </div>
            </article>
          {:else}
            <button class="empty-state" type="button" onclick={() => add('note')}><span class="empty-plus" aria-hidden="true">+</span><strong>No notes yet</strong><small>Capture an idea or useful context.</small></button>
          {/each}
        </div>
      </section>

      <section class="record-column task-column" aria-labelledby="tasks-heading">
        <div class="column-header">
          <div><span class="column-icon task-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M9 6h11M9 12h11M9 18h11M4 6l1 1 2-2M4 12l1 1 2-2M4 18l1 1 2-2" /></svg></span><strong id="tasks-heading">Tasks</strong><span class="record-count">{tasks.length}</span></div>
          <button class="add-button task-add" type="button" aria-label="Add task" title="Add task" disabled={busy} onclick={() => add('task')}><span aria-hidden="true">+</span></button>
        </div>
        <div class="record-list">
          {#each tasks as record (record.id)}
            <article>
              <div class="record-heading"><strong>{record.title}</strong><span class="record-status" data-status={record.status}>{statusLabel(record.status)}</span></div>
              {#if record.content}<p class="record-content">{record.content}</p>{/if}
              <div class="record-actions">
                {#if deletingId === record.id}
                  <span class="delete-prompt">Delete this task?</span>
                  <button class="text-button" type="button" disabled={busy} onclick={() => { deletingId = null; }}>Cancel</button>
                  <button class="text-button danger" type="button" disabled={busy} onclick={() => void remove(record)}>Delete</button>
                {:else}
                  <button class="text-button" type="button" disabled={busy} onclick={() => edit(record)}>Edit</button>
                  <button class="text-button danger" type="button" disabled={busy} onclick={() => { deletingId = record.id; }}>Delete</button>
                {/if}
              </div>
            </article>
          {:else}
            <button class="empty-state task-empty" type="button" onclick={() => add('task')}><span class="empty-plus" aria-hidden="true">+</span><strong>No tasks yet</strong><small>Add the next thing to get done.</small></button>
          {/each}
        </div>
      </section>
    </div>
  {/if}
</section>

<style>
  .records-panel { display: flex; flex-direction: column; min-width: 0; min-height: 420px; color: #e7eee9; background: #101512; }
  .panel-header { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 17px 18px; border-bottom: 1px solid #28322c; background: linear-gradient(180deg, #151c17, #111713); }
  .panel-title, .panel-actions, .panel-title > div, .column-header, .column-header > div, .record-heading, .record-actions, .editor-heading { display: flex; align-items: center; }
  .panel-title { gap: 11px; min-width: 0; }
  .panel-title > div { align-items: flex-start; flex-direction: column; gap: 3px; min-width: 0; }
  .panel-title strong { max-width: 420px; overflow: hidden; font-size: 15px; letter-spacing: -.01em; text-overflow: ellipsis; white-space: nowrap; }
  .panel-title small { color: #78857c; font-size: 9px; }
  .project-mark { width: 9px; height: 9px; border-radius: 3px; background: #8eefaa; box-shadow: 0 0 12px #8eefaa55; }
  .panel-actions { gap: 6px; }
  button { font: inherit; color: inherit; cursor: pointer; }
  button:disabled { opacity: .42; cursor: default; }
  .icon-button { display: grid; place-items: center; width: 29px; height: 29px; padding: 0; border: 1px solid #313d35; border-radius: 7px; color: #8c9990; background: #171f19; }
  .icon-button:hover:not(:disabled) { color: #cfe0d5; border-color: #4a5d50; background: #1c2720; }
  .icon-button svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
  .close-button:hover:not(:disabled) { color: #ff9da4; border-color: #744149; background: #271719; }
  .record-columns { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); min-height: 390px; }
  .record-column { min-width: 0; padding: 15px 16px 18px; }
  .record-column + .record-column { border-left: 1px solid #27312a; }
  .column-header { justify-content: space-between; margin-bottom: 12px; }
  .column-header > div { gap: 7px; }
  .column-header strong { font-size: 12px; letter-spacing: .01em; }
  .column-icon { display: grid; place-items: center; width: 25px; height: 25px; border: 1px solid #33433a; border-radius: 7px; color: #8eefaa; background: #142019; }
  .column-icon.task-icon { color: #8dd9ff; border-color: #304554; background: #121e25; }
  .column-icon svg { width: 13px; height: 13px; fill: none; stroke: currentColor; stroke-width: 1.7; stroke-linecap: round; stroke-linejoin: round; }
  .record-count { min-width: 19px; padding: 1px 6px; border-radius: 999px; color: #77837b; background: #202a23; font-size: 9px; font-weight: 750; line-height: 17px; text-align: center; }
  .add-button { display: grid; place-items: center; width: 27px; height: 27px; padding: 0; border: 1px solid #3e674a; border-radius: 7px; color: #9af0b3; background: #15231a; }
  .add-button:hover:not(:disabled) { border-color: #73cc8c; background: #1a2c20; transform: translateY(-1px); }
  .add-button span { font-size: 18px; font-weight: 350; line-height: 1; transform: translateY(-1px); }
  .add-button.task-add { color: #9bddff; border-color: #36586b; background: #14232b; }
  .add-button.task-add:hover:not(:disabled) { border-color: #70bfe8; background: #182c36; }
  .record-list { display: flex; flex-direction: column; gap: 8px; }
  article { padding: 12px; border: 1px solid #303b34; border-radius: 9px; background: #151c18; box-shadow: 0 2px 10px #0002; }
  article:hover { border-color: #405046; background: #18201b; }
  .record-heading { align-items: flex-start; justify-content: space-between; gap: 8px; }
  .record-heading strong { min-width: 0; overflow-wrap: anywhere; font-size: 11px; line-height: 1.45; }
  .record-content { margin: 7px 0 0; color: #9ca8a0; font-size: 10px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
  .record-status { flex: 0 0 auto; padding: 2px 7px; border: 1px solid #3b4740; border-radius: 999px; color: #9ba79f; background: #202923; font-size: 8px; font-weight: 700; }
  .record-status[data-status='in progress'] { color: #f0c56c; border-color: #69542d; background: #251f13; }
  .record-status[data-status='done'] { color: #8eefaa; border-color: #3a6947; background: #142219; }
  .record-actions { justify-content: flex-end; gap: 4px; margin-top: 9px; }
  .text-button { padding: 2px 4px; border: 0; color: #718078; background: transparent; font-size: 8px; }
  .text-button:hover:not(:disabled) { color: #b7c3bb; }
  .text-button.danger:hover:not(:disabled) { color: #f09aa1; }
  .delete-prompt { margin-right: auto; color: #d6969b; font-size: 8px; }
  .empty-state { display: grid; place-items: center; width: 100%; min-height: 176px; padding: 28px 18px; border: 1px dashed #334139; border-radius: 10px; color: #728078; background: #121814; text-align: center; }
  .empty-state:hover { color: #94a198; border-color: #4c6756; background: #151e18; }
  .empty-state strong { margin-top: 10px; color: #a8b4ac; font-size: 11px; }
  .empty-state small { margin-top: 4px; color: #637067; font-size: 9px; }
  .empty-plus { display: grid; place-items: center; width: 34px; height: 34px; border: 1px solid #3d5e48; border-radius: 10px; color: #8eefaa; background: #16231a; font-size: 22px; font-weight: 300; }
  .task-empty .empty-plus { color: #8dd9ff; border-color: #36576a; background: #14232a; }
  .record-error { margin: 12px 16px 0; padding: 8px 10px; border: 1px solid #723c42; border-radius: 7px; color: #ffadb3; background: #2c171a; font-size: 10px; }
  .loading-state { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 390px; color: #7f8c83; font-size: 10px; }
  .loading-state span { width: 12px; height: 12px; border: 2px solid #344239; border-top-color: #8eefaa; border-radius: 50%; animation: spin .8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .record-editor { display: grid; gap: 14px; margin: 18px; padding: 18px; border: 1px solid #344139; border-radius: 11px; background: #131a16; }
  .editor-heading { justify-content: space-between; padding-bottom: 13px; border-bottom: 1px solid #28332c; }
  .editor-heading > div { display: grid; gap: 3px; }
  .editor-heading strong { font-size: 13px; }
  .editor-kicker { color: #78857c; font-size: 8px; font-weight: 750; letter-spacing: .12em; text-transform: uppercase; }
  .kind-badge { padding: 4px 8px; border-radius: 999px; color: #8eefaa; background: #1b2b20; font-size: 8px; font-weight: 750; }
  .kind-badge.task-accent { color: #8dd9ff; background: #182831; }
  .record-editor label { display: grid; gap: 6px; color: #85928a; font-size: 8px; font-weight: 750; letter-spacing: .08em; text-transform: uppercase; }
  .record-editor input, .record-editor textarea, .record-editor select { width: 100%; box-sizing: border-box; padding: 9px 10px; border: 1px solid #35443a; border-radius: 7px; outline: none; color: #e4ebe6; background: #0c120e; font: inherit; font-size: 11px; text-transform: none; }
  .record-editor input { height: 36px; }
  .record-editor textarea { resize: vertical; line-height: 1.5; }
  .record-editor input:focus, .record-editor textarea:focus, .record-editor select:focus { border-color: #72c98a; box-shadow: 0 0 0 2px #72c98a18; }
  .record-editor .record-actions { margin-top: 0; }
  .secondary-button, .primary-button { min-width: 78px; height: 31px; padding: 0 12px; border-radius: 7px; font-size: 10px; font-weight: 700; }
  .secondary-button { border: 1px solid #354139; color: #aab5ad; background: #18201a; }
  .primary-button { border: 0; color: #0b140e; background: #8eefaa; }
  @media (max-width: 620px) {
    .record-columns { grid-template-columns: 1fr; }
    .record-column + .record-column { border-top: 1px solid #27312a; border-left: 0; }
    .panel-title strong { max-width: 260px; }
  }
</style>
