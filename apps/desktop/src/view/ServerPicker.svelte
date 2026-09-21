<script lang="ts">
  import type { DesktopClient, SshHost } from '../application/desktop-client';
  import type { CreateServerInput, ManagedServer, UpdateServerInput } from '../domain/session';

  type AuthKind = 'password' | 'keyFile' | 'generateKey';

  let {
    sshHosts,
    managedServers,
    generatedKey,
    client,
    onSelectSshHost,
    onSelectManagedServer,
    onCreateServer,
    onUpdateServer,
    onDeleteServer,
    onDismissGeneratedKey,
    onClose,
  }: {
    sshHosts: SshHost[];
    managedServers: ManagedServer[];
    generatedKey: { serverId: string; publicKey: string } | null;
    client: DesktopClient;
    onSelectSshHost: (alias: string) => void;
    onSelectManagedServer: (server: ManagedServer) => void;
    onCreateServer: (input: CreateServerInput) => Promise<void>;
    onUpdateServer: (input: UpdateServerInput) => Promise<void>;
    onDeleteServer: (id: string) => void;
    onDismissGeneratedKey: () => void;
    onClose: () => void;
  } = $props();

  let showForm = $state(false);
  let editingId = $state<string | null>(null);
  let name = $state('');
  let host = $state('');
  let port = $state('');
  let username = $state('');
  let authKind = $state<AuthKind>('password');
  let password = $state('');
  let showPassword = $state(false);
  let keyPath = $state('');
  let submitting = $state(false);
  let formError = $state<string | null>(null);
  let copied = $state(false);

  function resetForm(): void {
    editingId = null;
    name = '';
    host = '';
    port = '';
    username = '';
    authKind = 'password';
    password = '';
    showPassword = false;
    keyPath = '';
    formError = null;
  }

  function openAddForm(): void {
    resetForm();
    showForm = true;
  }

  function openEditForm(server: ManagedServer): void {
    editingId = server.id;
    name = server.name;
    host = server.host;
    port = String(server.port);
    username = server.username;
    authKind = server.auth.kind === 'keyFile' ? 'keyFile' : 'password';
    password = '';
    keyPath = server.auth.kind === 'keyFile' ? server.auth.path : '';
    formError = null;
    showForm = true;
  }

  async function pickKeyFile(): Promise<void> {
    const selected = await client.selectFile(keyPath || undefined);
    if (selected) keyPath = selected;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!name.trim() || !host.trim() || !username.trim()) {
      formError = 'Name, host, and username are required.';
      return;
    }
    // Editing with the password field left blank means "keep the existing
    // one" — the normal pattern for a credential you can't read back to
    // prefill. Creating a server has no existing credential to keep, so a
    // password is required there.
    if (authKind === 'password' && !password && !editingId) {
      formError = 'Enter a password.';
      return;
    }
    if (authKind === 'keyFile' && !keyPath.trim()) {
      formError = 'Choose a key file.';
      return;
    }
    const auth =
      authKind === 'password'
        ? (password ? { kind: 'password' as const, password } : { kind: 'unchanged' as const })
      : authKind === 'keyFile' ? { kind: 'keyFile' as const, path: keyPath.trim() }
      : { kind: 'generateKey' as const };
    submitting = true;
    formError = null;
    try {
      const base = {
        name: name.trim(),
        host: host.trim(),
        port: port.trim() ? Number(port.trim()) : undefined,
        username: username.trim(),
      };
      if (editingId) await onUpdateServer({ id: editingId, ...base, auth });
      else await onCreateServer({ ...base, auth: auth.kind === 'unchanged' ? { kind: 'password', password: '' } : auth });
      resetForm();
      showForm = false;
    } finally {
      submitting = false;
    }
  }

  async function copyPublicKey(): Promise<void> {
    if (!generatedKey) return;
    await navigator.clipboard.writeText(generatedKey.publicKey);
    copied = true;
  }
</script>

<div class="folder-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
  <div class="server-panel" role="dialog" aria-modal="true" aria-label="Choose a remote server">
    <header>
      <div><strong>Remote server</strong><small>Pick a server to connect over SSH</small></div>
      <button class="icon-button" aria-label="Close" onclick={onClose}>×</button>
    </header>
    <div class="server-columns">
      <section class="server-column" aria-label="~/.ssh/config">
        <p class="eyebrow">~/.ssh/config</p>
        {#if sshHosts.length}
          <div class="server-list">
            {#each sshHosts as sshHost (sshHost.alias)}
              <button class="server-row" onclick={() => onSelectSshHost(sshHost.alias)}>{sshHost.alias}</button>
            {/each}
          </div>
        {:else}
          <p class="folder-state">No hosts found.</p>
        {/if}
      </section>
      <div class="server-divider" aria-hidden="true"></div>
      <section class="server-column" aria-label="Saved servers">
        <p class="eyebrow">Saved servers <button class="icon-button add-server" aria-label="Add server" onclick={() => { if (showForm && !editingId) { showForm = false; } else openAddForm(); }}>+</button></p>
        {#if generatedKey}
          <div class="generated-key">
            <p>Server saved. Add this public key to the remote's <code>~/.ssh/authorized_keys</code>. It won't be shown again once you close this.</p>
            <textarea readonly rows="3">{generatedKey.publicKey}</textarea>
            <div class="generated-key-actions">
              <button class="primary compact" onclick={copyPublicKey}>{copied ? 'Copied ✓' : 'Copy public key'}</button>
              <button onclick={onDismissGeneratedKey}>Close</button>
            </div>
          </div>
        {/if}
        {#if showForm}
          <form class="add-server-form" onsubmit={submit}>
            <label>Name<input bind:value={name} placeholder="my-server" required /></label>
            <label>Domain / IP<input bind:value={host} placeholder="1.2.3.4" required /></label>
            <label>Port<input bind:value={port} placeholder="22" inputmode="numeric" /></label>
            <label>Username<input bind:value={username} placeholder="root" required /></label>
            <div class="auth-choice" role="radiogroup" aria-label="Authentication">
              <label class="auth-option"><input type="radio" name="auth" value="password" checked={authKind === 'password'} onchange={() => authKind = 'password'} />Password</label>
              <label class="auth-option"><input type="radio" name="auth" value="keyFile" checked={authKind === 'keyFile'} onchange={() => authKind = 'keyFile'} />Key file</label>
              <label class="auth-option"><input type="radio" name="auth" value="generateKey" checked={authKind === 'generateKey'} onchange={() => authKind = 'generateKey'} />Generate new key</label>
            </div>
            {#if authKind === 'password'}
              <label class="wide">Password
                <div class="password-actions">
                  <input type={showPassword ? 'text' : 'password'} bind:value={password} autocomplete="off" placeholder={editingId ? 'Leave blank to keep the current password' : ''} />
                  <button type="button" class="folder-button" aria-label={showPassword ? 'Hide password' : 'Show password'} title={showPassword ? 'Hide password' : 'Show password'} onclick={() => showPassword = !showPassword}>{showPassword ? 'Hide' : 'Show'}</button>
                </div>
              </label>
            {:else if authKind === 'keyFile'}
              <label class="wide">Key file<div class="path-actions"><input value={keyPath} readonly placeholder="~/.ssh/id_ed25519" /><button class="folder-button" type="button" aria-label="Choose key file" onclick={pickKeyFile}>⌂</button></div></label>
              {#if editingId}<p class="field-hint wide">Already set to the current key file; choose a different one to replace it.</p>{/if}
            {:else}
              <p class="field-hint wide">Generates a new ed25519 key on save, and shows the public key once so you can copy it.</p>
            {/if}
            {#if formError}<p class="field-hint wide server-form-error">{formError}</p>{/if}
            <div class="editor-actions wide"><button type="button" onclick={() => { showForm = false; resetForm(); }}>Cancel</button><button class="primary" type="submit" disabled={submitting}>{submitting ? 'Saving…' : editingId ? 'Save' : 'Create'}</button></div>
          </form>
        {/if}
        {#if managedServers.length}
          <div class="server-list">
            {#each managedServers as server (server.id)}
              <div class="server-row-wrap">
                <button class="server-row" onclick={() => onSelectManagedServer(server)}>
                  <strong>{server.name}</strong><small>{server.username}@{server.host}:{server.port} · {server.auth.kind === 'password' ? 'password' : 'key'}</small>
                </button>
                <button class="server-edit" aria-label={`Edit ${server.name}`} title="Edit" onclick={(event) => { event.stopPropagation(); openEditForm(server); }}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zm8.9 2.4-1.6-.4a7.3 7.3 0 0 0-.5-1.2l.9-1.4a1 1 0 0 0-.1-1.2l-1.3-1.3a1 1 0 0 0-1.2-.1l-1.4.9a7.3 7.3 0 0 0-1.2-.5l-.4-1.6a1 1 0 0 0-1-.8h-1.8a1 1 0 0 0-1 .8l-.4 1.6a7.3 7.3 0 0 0-1.2.5l-1.4-.9a1 1 0 0 0-1.2.1L3.9 6.7a1 1 0 0 0-.1 1.2l.9 1.4a7.3 7.3 0 0 0-.5 1.2l-1.6.4a1 1 0 0 0-.8 1v1.8a1 1 0 0 0 .8 1l1.6.4c.13.43.3.83.5 1.2l-.9 1.4a1 1 0 0 0 .1 1.2l1.3 1.3a1 1 0 0 0 1.2.1l1.4-.9c.37.2.77.37 1.2.5l.4 1.6a1 1 0 0 0 1 .8h1.8a1 1 0 0 0 1-.8l.4-1.6c.43-.13.83-.3 1.2-.5l1.4.9a1 1 0 0 0 1.2-.1l1.3-1.3a1 1 0 0 0 .1-1.2l-.9-1.4c.2-.37.37-.77.5-1.2l1.6-.4a1 1 0 0 0 .8-1v-1.8a1 1 0 0 0-.8-1z" /></svg>
                </button>
                <button class="server-delete" aria-label={`Delete ${server.name}`} title="Delete" onclick={(event) => { event.stopPropagation(); onDeleteServer(server.id); }}>×</button>
              </div>
            {/each}
          </div>
        {:else if !showForm}
          <p class="folder-state">Use + to add a server.</p>
        {/if}
      </section>
    </div>
  </div>
</div>
