<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { createDesktopClient } from '../application/client-provider';
  import { type CliKind, type WebAgent } from '../domain/session';
  import { snapshotAge, unlistedAgentIds } from '../domain/usage-snapshots';
  import { AppViewModel } from '../viewmodel/app-view-model.svelte';
  import { TauriSessionRecordRepository } from '../infrastructure/tauri-session-record-repository';
  import RemoteFolderPicker from './RemoteFolderPicker.svelte';
  import ServerPicker from './ServerPicker.svelte';
  import SessionRecords from './SessionRecords.svelte';
  import TerminalPane from './TerminalPane.svelte';

  const recordRepository = new TauriSessionRecordRepository();
  const viewModel = new AppViewModel(createDesktopClient(), recordRepository);
  const recordSyncNote = $derived(
    !viewModel.authenticated ? 'Stored on this device only. Sign in to back up and sync.'
      : viewModel.recordSyncError ? `Not synced: ${viewModel.recordSyncError}`
      : 'Synced with your account.',
  );
  let recordsTarget = $state<{ profileId: string; name: string } | null>(null);
  // Open (not done) tasks per saved session profile, for the badge on the
  // notes button. Reloads when records change from a sync, MCP, or the dialog.
  let openTaskCounts = $state<Record<string, number>>({});
  let localRecordsRevision = $state(0);
  $effect(() => {
    void viewModel.recordsRevision;
    void localRecordsRevision;
    let current = true;
    void recordRepository.listAll().then((records) => {
      if (!current) return;
      const counts: Record<string, number> = {};
      for (const record of records) {
        if (record.kind === 'task' && record.status !== 'done') counts[record.profileId] = (counts[record.profileId] ?? 0) + 1;
      }
      openTaskCounts = counts;
    }).catch(() => {});
    return () => { current = false; };
  });
  const poppedSessionId = new URLSearchParams(window.location.search).get('session');
  const cliNames: Record<CliKind, string> = { codex: 'Codex', gemini: 'Gemini', claude: 'Claude Code' };
  const activityLabels: Record<'working' | 'waiting' | 'idle', string> = {
    working: 'Working — agent is actively producing output',
    waiting: 'Needs you — approval or confirmation required',
    idle: 'Ready — no work in progress',
  };
  // A `failed` session (its shell process died — most commonly an SSH
  // session losing its network connection) stays visible with a resume
  // affordance instead of vanishing; only an explicitly `completed` shell
  // (the user typed `exit`) drops out of the grid on its own.
  const visibleSessions = $derived(
    poppedSessionId
      ? viewModel.sessions.filter((session) => session.id === poppedSessionId)
      : (viewModel.selectedGroup?.sessions ?? []).filter((session) => session.status !== 'completed'),
  );
  // Keep every live terminal mounted while switching groups. Recreating an
  // xterm from a truncated raw PTY transcript cannot reconstruct a full-screen
  // TUI such as Codex; hiding it preserves the terminal's actual screen state.
  const renderedSessions = $derived(
    poppedSessionId
      ? visibleSessions
      : viewModel.sessions.filter((session) => session.status !== 'completed'),
  );
  let sessionMenu = $state<{ sessionId: string; x: number; y: number } | null>(null);
  let groupMenu = $state<{ groupId: string; groupName: string; x: number; y: number } | null>(null);
  let groupRename = $state<{ groupId: string; name: string } | null>(null);
  let collapsedGroups = $state<Record<string, boolean>>({});
  let ungroupedOpen = $state(true);
  let usageOpen = $state(true);
  let clockNow = $state(new Date());
  let editingAgentId = $state<string | null>(null);
  let agentEditorOpen = $state(false);
  let agentDraft = $state<WebAgent>(emptyAgent());
  let terminalLayoutRevision = $state(0);

  function emptyAgent(): WebAgent {
    return { id: `web-${crypto.randomUUID()}`, name: '', chatUrl: '', usageUrl: '', accent: '#8eefaa', enabled: true };
  }

  function editAgent(agent?: WebAgent): void {
    const selected = agent ?? emptyAgent();
    editingAgentId = agent?.id ?? null;
    agentDraft = { ...selected };
    agentEditorOpen = Boolean(agent);
  }

  function addAgent(): void {
    editingAgentId = null;
    agentDraft = emptyAgent();
    agentEditorOpen = true;
  }

  function closeAgentEditor(): void {
    editingAgentId = null;
    agentDraft = emptyAgent();
    agentEditorOpen = false;
  }

  function saveAgent(): void {
    viewModel.saveWebAgent({ ...agentDraft, id: editingAgentId ?? agentDraft.id });
    closeAgentEditor();
  }

  function formatClock(date: Date, timeZone?: string): string {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false, timeZone });
  }

  function formatReset(timestamp?: number): string {
    if (!timestamp) return '';
    const milliseconds = Math.max(0, timestamp * 1000 - Date.now());
    const hours = Math.floor(milliseconds / 3_600_000);
    const minutes = Math.floor((milliseconds % 3_600_000) / 60_000);
    return hours >= 24 ? `${Math.floor(hours / 24)}d ${hours % 24}h` : `${hours}h ${minutes}m`;
  }

  async function focusTerminal(sessionId: string): Promise<void> {
    await tick();
    document.querySelector<HTMLElement>(`[data-session-id="${CSS.escape(sessionId)}"] .xterm-helper-textarea`)?.focus();
  }

  function handleGlobalKeydown(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && ['=', '+', '-', '_', '0'].includes(event.key)) {
      event.preventDefault();
      if (event.key === '0') viewModel.resetFontSize();
      else if (event.key === '-' || event.key === '_') viewModel.decreaseFontSize();
      else viewModel.increaseFontSize();
      return;
    }
    if (!poppedSessionId && event.ctrlKey && event.key === 'Tab') {
      event.preventDefault();
      event.stopImmediatePropagation();
      const sessionId = viewModel.cycleSession(event.shiftKey);
      if (sessionId) void focusTerminal(sessionId);
      return;
    }
    if (event.key !== 'Escape') return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (groupRename) {
      groupRename = null;
      return;
    }
    if (sessionMenu || groupMenu) {
      closeContextMenus();
      return;
    }
    if (recordsTarget) {
      recordsTarget = null;
      return;
    }
    if (viewModel.settingsOpen) {
      viewModel.toggleSettings();
      return;
    }
    const selected = viewModel.sessions.find((session) => session.id === viewModel.selectedSessionId);
    if (selected) void viewModel.interruptSession();
  }

  function openSessionMenu(event: MouseEvent, sessionId: string): void {
    event.preventDefault();
    event.stopPropagation();
    groupMenu = null;
    sessionMenu = { sessionId, x: event.clientX, y: event.clientY };
  }

  function openGroupMenu(event: MouseEvent, groupId: string, groupName: string): void {
    event.preventDefault();
    event.stopPropagation();
    sessionMenu = null;
    groupMenu = { groupId, groupName, x: event.clientX, y: event.clientY };
  }

  function renameFromMenu(): void {
    if (!sessionMenu) return;
    const session = viewModel.sessions.find((item) => item.id === sessionMenu?.sessionId);
    sessionMenu = null;
    if (!session) return;
    const title = window.prompt('Session name', session.title);
    if (title?.trim()) void viewModel.renameSession(session.id, title);
  }

  function renameGroupFromMenu(): void {
    if (!groupMenu) return;
    const { groupId, groupName } = groupMenu;
    groupMenu = null;
    groupRename = { groupId, name: groupName };
  }

  function saveGroupName(): void {
    if (!groupRename?.name.trim()) return;
    viewModel.renameGroup(groupRename.groupId, groupRename.name);
    groupRename = null;
  }

  function closeContextMenus(): void {
    sessionMenu = null;
    groupMenu = null;
  }

  function toggleGroup(groupId: string): void {
    collapsedGroups = { ...collapsedGroups, [groupId]: !isGroupCollapsed(groupId) };
  }

  function isGroupCollapsed(groupId: string): boolean {
    return collapsedGroups[groupId] ?? true;
  }

  function handleGlobalContextMenu(event: MouseEvent): void {
    event.preventDefault();
  }

  // WKWebView is picky about DataTransfer for in-document drags (custom MIME
  // types, and drags that begin on a <button>), so remember the dragged
  // session ourselves instead of relying on getData() in the drop handler.
  // $state so the trash drop zone can appear as soon as a drag starts.
  let draggingSessionId = $state<string | null>(null);
  let trashHover = $state(false);

  function startSessionDrag(event: DragEvent, sessionId: string): void {
    draggingSessionId = sessionId;
    event.dataTransfer?.setData('text/plain', sessionId);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
  }

  function draggedSessionId(event: DragEvent): string | null {
    return draggingSessionId ?? event.dataTransfer?.getData('text/plain') ?? null;
  }

  function handleTrashDragOver(event: DragEvent): void {
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    trashHover = true;
  }

  function handleTrashDrop(event: DragEvent): void {
    event.preventDefault();
    trashHover = false;
    const entryId = draggedSessionId(event);
    draggingSessionId = null;
    if (entryId) void viewModel.deleteEntry(entryId);
  }

  function handleActiveGroupDragOver(event: DragEvent): void {
    if (poppedSessionId) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
  }

  function handleActiveGroupDrop(event: DragEvent): void {
    if (poppedSessionId) return;
    event.preventDefault();
    const sessionId = draggedSessionId(event);
    draggingSessionId = null;
    if (sessionId) void viewModel.moveSessionToGroup(sessionId, viewModel.selectedGroupId);
  }

  // Another window (a popped-out session) can close or restart sessions;
  // re-check the backend whenever this window comes back to the front.
  const syncSessionStatuses = (): void => { void viewModel.syncSessionStatuses(); };

  onMount(() => {
    let disposed = false;
    let terminalLayoutFrame = 0;
    const nativeWindowUnlisteners: Array<() => void> = [];
    const clockInterval = window.setInterval(() => { clockNow = new Date(); }, 30_000);
    const notifyTerminalLayoutChanged = (): void => {
      cancelAnimationFrame(terminalLayoutFrame);
      terminalLayoutFrame = requestAnimationFrame(() => { terminalLayoutRevision += 1; });
    };
    void Promise.allSettled([
      getCurrentWindow().onResized(notifyTerminalLayoutChanged),
      getCurrentWindow().onScaleChanged(notifyTerminalLayoutChanged),
    ]).then((results) => {
      const unlisteners = results.flatMap((result) => result.status === 'fulfilled' ? [result.value] : []);
      if (disposed) unlisteners.forEach((unlisten) => unlisten());
      else nativeWindowUnlisteners.push(...unlisteners);
    });
    window.addEventListener('keydown', handleGlobalKeydown, { capture: true });
    window.addEventListener('click', closeContextMenus);
    window.addEventListener('contextmenu', handleGlobalContextMenu, { capture: true });
    window.addEventListener('focus', syncSessionStatuses);
    void viewModel.load({ auxiliaryWindow: Boolean(poppedSessionId) });
    return () => {
      disposed = true;
      window.clearInterval(clockInterval);
      cancelAnimationFrame(terminalLayoutFrame);
      nativeWindowUnlisteners.forEach((unlisten) => unlisten());
      window.removeEventListener('keydown', handleGlobalKeydown, { capture: true });
      window.removeEventListener('click', closeContextMenus);
      window.removeEventListener('contextmenu', handleGlobalContextMenu, { capture: true });
      window.removeEventListener('focus', syncSessionStatuses);
      viewModel.dispose();
    };
  });
</script>

<svelte:head><title>fastade</title></svelte:head>

{#snippet hostRows(agentId: string, name: string, accent: string)}
  {#each viewModel.hostUsage.filter((host) => host.agentId === agentId) as host (host.hostId)}
    {@const age = snapshotAge(host.collectedAt, Math.floor(clockNow.getTime() / 1000))}
    <div class="agent-usage-row" class:stale={age.stale} style={`--agent-accent:${accent}`} title={`${host.hostLabel} · updated ${age.label}`}>
      <div class="agent-launch"><span class="agent-dot"></span><strong>{name}</strong></div>
      <div class="usage-host-head"><span class="usage-host-name">{host.hostLabel}</span><small>{age.label}</small></div>
      {#each host.windows as window}
        <div class="usage-meter" title={`${window.label} ${window.remainingPercent}% remaining`}>
          <span>{window.label}</span><div><i style={`width:${window.remainingPercent}%`}></i></div><b>{window.remainingPercent}%</b><small title={window.resetText ?? ''}>{window.resetText ?? formatReset(window.resetsAt)}</small>
        </div>
      {/each}
    </div>
  {/each}
{/snippet}

<div class:drawer-closed={!viewModel.drawerOpen || poppedSessionId} class="shell">
  {#if viewModel.drawerOpen && !poppedSessionId}
    <aside aria-label="Session drawer">
      <header class="brand">
        <span class="brand-word"><em>fast</em>ade</span>
        <span class="brand-version">v{__APP_VERSION__}</span>
        <button class="icon-button settings-button" class:authenticated={viewModel.authenticated} onclick={() => viewModel.toggleSettings()} aria-label="Settings" title={viewModel.authenticated ? 'Settings · signed in' : 'Settings'}>⚙</button>
        <button class="icon-button drawer-close" onclick={() => viewModel.toggleDrawer()} aria-label="Close drawer">‹</button>
      </header>
      <section class="agent-status-panel" aria-label="AI usage">
        <div class="agent-status-heading">
          <button class="section-heading" aria-expanded={usageOpen} onclick={() => { usageOpen = !usageOpen; }}>
            <span>AI USAGE</span>
            <small class="usage-clock" title="Local time · UTC">{formatClock(clockNow)} · {formatClock(clockNow, 'UTC')} UTC</small>
            <span aria-hidden="true">{usageOpen ? '⌃' : '⌄'}</span>
          </button>
          <button title="Refresh usage" aria-label="Refresh usage" onclick={() => void viewModel.refreshAgentUsage(true)}>↻</button>
        </div>
        {#if usageOpen}
          {#each viewModel.webAgents.filter((agent) => agent.enabled) as agent (agent.id)}
            {@const usage = viewModel.agentUsage[agent.id]}
            <div class="agent-usage-row" style={`--agent-accent:${agent.accent}`}>
              <div class="agent-launch"><span class="agent-dot"></span><strong>{agent.name}</strong></div>
              {#if usage?.windows.length}
                {#each usage.windows as window}
                  <div class="usage-meter" title={`${window.label} ${window.remainingPercent}% remaining`}>
                    <span>{window.label}</span><div><i style={`width:${window.remainingPercent}%`}></i></div><b>{window.remainingPercent}%</b><small title={window.resetText ?? ''}>{window.resetText ?? formatReset(window.resetsAt)}</small>
                  </div>
                {/each}
              {:else}
                <div class="usage-unavailable"><span>{usage?.message ?? 'Checking…'}</span></div>
              {/if}
            </div>
            {@render hostRows(agent.id, agent.name, agent.accent)}
          {/each}
          {#each unlistedAgentIds(viewModel.hostUsage, viewModel.webAgents.filter((agent) => agent.enabled).map((agent) => agent.id)) as agentId (agentId)}
            {@render hostRows(agentId, viewModel.webAgents.find((agent) => agent.id === agentId)?.name ?? agentId, '#718078')}
          {/each}
        {/if}
      </section>
      <div class="server-launch">
        <button class="primary" onclick={() => void viewModel.startLocalShell()}>Local</button>
        <button class="primary secondary" onclick={() => viewModel.openServerPicker()}>Remote</button>
      </div>
      {#if viewModel.ungroupedProfiles.length}
        <section class="ungrouped-profiles" aria-label="Ungrouped">
          <button class="section-heading" aria-expanded={ungroupedOpen} onclick={() => { ungroupedOpen = !ungroupedOpen; }}>
            <span>UNGROUPED · {viewModel.ungroupedProfiles.length}</span><span aria-hidden="true">{ungroupedOpen ? '⌃' : '⌄'}</span>
          </button>
          {#if ungroupedOpen}
            {#each viewModel.ungroupedProfiles as profile (profile.id)}
              {@const path = viewModel.ungroupedPathFor(profile)}
              <div class="session-row">
                <button class="session-select" title={`Start a shell on ${viewModel.endpointLabel(profile.lastEndpoint)}:${path}`} onclick={() => void viewModel.startSaved(profile)}>
                  <span class="status" data-status="disconnected"></span><span><strong>{profile.name}</strong><small>{profile.lastEndpoint === 'local' ? 'local' : `ssh:${viewModel.endpointLabel(profile.lastEndpoint)}`} · {path}</small></span>
                </button>
                <button
                  class="entry-action remove"
                  title="Remove"
                  aria-label={`Remove ${profile.name}`}
                  onclick={() => void viewModel.removeUngroupedProfile(profile.id)}
                >×</button>
              </div>
            {/each}
          {/if}
        </section>
      {/if}
      <nav aria-label="Group list">
        <p class="eyebrow">GROUPS · {viewModel.sessionGroups.length - 1}</p>
        <form class="new-group" onsubmit={(event) => { event.preventDefault(); viewModel.createGroup(); }}>
          <input bind:value={viewModel.draftGroupName} aria-label="New group name" placeholder="New group" />
          <button type="submit" aria-label="Add group" title="Add group">+</button>
        </form>
        {#each viewModel.sessionGroups as group (group.id)}
          {@const groupTasks = [...new Set(group.entries.flatMap((entry) => entry.profileId ?? []))].reduce((sum, id) => sum + (openTaskCounts[id] ?? 0), 0)}
          <section
            role="group"
            aria-label={`${group.name} session group`}
            class:active={group.id === viewModel.selectedGroupId}
            class="group-block"
            ondragover={(event) => { event.preventDefault(); if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'; }}
            ondrop={(event) => {
              event.preventDefault();
              const sessionId = draggedSessionId(event);
              draggingSessionId = null;
              if (sessionId) void viewModel.moveSessionToGroup(sessionId, group.id);
            }}
          >
            <div class="group-header">
              <button class="group-select" aria-pressed={group.id === viewModel.selectedGroupId} onclick={() => viewModel.selectGroup(group.id)} oncontextmenu={(event) => openGroupMenu(event, group.id, group.name)}>
                <strong>{group.name}</strong>
                {#if groupTasks}<span class="group-count" aria-label={`${groupTasks} open tasks`} title={`${groupTasks} open tasks in this group`}>{groupTasks}</span>{/if}
              </button>
              <button
                class="group-collapse-target"
                aria-expanded={!isGroupCollapsed(group.id)}
                aria-label={`${isGroupCollapsed(group.id) ? 'Expand' : 'Collapse'} ${group.name} sessions`}
                title={`${isGroupCollapsed(group.id) ? 'Expand' : 'Collapse'} sessions`}
                onclick={() => toggleGroup(group.id)}
              ></button>
              <div class="group-actions">
                <button
                  class="group-toggle"
                  aria-expanded={!isGroupCollapsed(group.id)}
                  aria-label={`${isGroupCollapsed(group.id) ? 'Expand' : 'Collapse'} ${group.name} sessions`}
                  title={`${isGroupCollapsed(group.id) ? 'Expand' : 'Collapse'} sessions`}
                  onclick={() => toggleGroup(group.id)}
                >{isGroupCollapsed(group.id) ? '›' : '⌄'}</button>
                {#if group.custom}
                  <button class="group-delete" aria-label={`Delete ${group.name}`} title={`Delete group (sessions move to ${viewModel.defaultGroupName})`} onclick={() => viewModel.deleteGroup(group.id)}>×</button>
                {/if}
              </div>
            </div>
            {#if !isGroupCollapsed(group.id)}
              <div class="group-sessions">
                {#each group.entries as entry (entry.id)}
                  <div
                    role="group"
                    aria-label={`${entry.title} session`}
                    class:focused={entry.sessionId === viewModel.selectedSessionId}
                    class="session-row"
                    draggable="true"
                    oncontextmenu={(event) => { if (entry.sessionId) openSessionMenu(event, entry.sessionId); }}
                    ondragstart={(event) => startSessionDrag(event, entry.id)}
                    ondragend={() => { draggingSessionId = null; }}
                  >
                    <button class="session-select" draggable="true" ondragstart={(event) => startSessionDrag(event, entry.id)} onclick={() => void viewModel.openGroupEntry(entry, group.id)}>
                      {#if entry.cli && entry.activity}
                        <span class="activity" data-activity={entry.activity} title={activityLabels[entry.activity]} aria-label={activityLabels[entry.activity]}></span>
                      {:else}
                        <span class="status" data-status={entry.status}></span>
                      {/if}
                      <span><strong>{entry.title}{#if entry.profileId && openTaskCounts[entry.profileId]}<span class="task-count" aria-label={`${openTaskCounts[entry.profileId]} open tasks`} title={`${openTaskCounts[entry.profileId]} open tasks`}>{openTaskCounts[entry.profileId]}</span>{/if}</strong><small>{entry.endpoint === 'local' ? 'local' : `ssh:${viewModel.endpointLabel(entry.endpoint)}`} · {entry.projectPath}{entry.cli ? ` · ${cliNames[entry.cli]}` : ''}</small></span>
                    </button>
                    {#if entry.sessionId}
                      {@const session = viewModel.sessions.find((item) => item.id === entry.sessionId)}
                      {#if session}
                        <div class="session-row-actions">
                          {#if !group.custom && !viewModel.hasUngroupedProfile(session)}
                            <button class="entry-action add" disabled={!viewModel.hasDesignatedPath(session)} aria-label={`Keep ${session.title} in Ungrouped`} title={viewModel.hasDesignatedPath(session) ? `Keep in Ungrouped: ${viewModel.endpointLabel(session.endpoint)}:${session.projectPath}` : 'Set a folder with ⌂ first'} onclick={(event) => { event.stopPropagation(); void viewModel.toggleUngroupedProfile(session); }}>+</button>
                          {/if}
                          <button
                            class="entry-action pin"
                            class:active={viewModel.isSessionPinned(session.id)}
                            aria-pressed={viewModel.isSessionPinned(session.id)}
                            aria-label={viewModel.isSessionPinned(session.id) ? `Unpin ${session.title}` : `Pin ${session.title}`}
                            title={viewModel.isSessionPinned(session.id) ? 'Do not restore on next launch' : 'Restore this session on next launch'}
                            onclick={(event) => { event.stopPropagation(); viewModel.toggleSessionPin(session.id); }}
                          ><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M14 4.5v4l2.5 2.5v1.5h-3.75V19L12 20.5 11.25 19v-6.5H7.5V11L10 8.5v-4z" /></svg></button>
                        </div>
                      {/if}
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          </section>
        {/each}
      </nav>
    </aside>
  {/if}

  <main>
    {#if !viewModel.drawerOpen && !poppedSessionId}<button class="icon-button drawer-opener" onclick={() => viewModel.toggleDrawer()} aria-label="Open drawer">☰</button>{/if}
    {#if viewModel.error}<div class="error" role="alert">{viewModel.error}</div>{/if}
    {#if !poppedSessionId && !viewModel.loading && !visibleSessions.length}
      <div
        role="group"
        aria-label="Move session to the active group"
        class="active-group-dropbar"
        ondragover={handleActiveGroupDragOver}
        ondrop={handleActiveGroupDrop}
      >Drop here → <strong>{viewModel.selectedGroup?.name ?? 'Sessions'}</strong></div>
    {/if}
    {#if viewModel.loading}
      <div class="empty">Loading sessions…</div>
    {:else if renderedSessions.length}
      <section
        role="group"
        aria-label="Active group sessions"
        class:single={poppedSessionId}
        class:three={visibleSessions.length === 3 && !poppedSessionId}
        class:four={visibleSessions.length === 4 && !poppedSessionId}
        class="session-grid"
        ondragover={handleActiveGroupDragOver}
        ondrop={handleActiveGroupDrop}
      >
        {#each renderedSessions as session (session.id)}
          {@const agentRunning = viewModel.isAgentRunning(session.id)}
          {@const activity = viewModel.activityFor(session.id)}
          {@const memoryLabel = viewModel.memoryLabelFor(session.id)}
          {@const recordProfile = viewModel.profileForSession(session)}
          {@const openTasks = recordProfile ? openTaskCounts[recordProfile.id] ?? 0 : 0}
          <article data-session-id={session.id} hidden={!poppedSessionId && !visibleSessions.some((visible) => visible.id === session.id)} role="group" aria-label={`${session.title} terminal session`} class:active={session.id === viewModel.selectedSessionId} class:grid-left={visibleSessions.length === 3 && !poppedSessionId && visibleSessions.at(-1)?.id === session.id} class:grid-right={visibleSessions.length === 3 && !poppedSessionId && visibleSessions.at(-1)?.id !== session.id && visibleSessions.some((visible) => visible.id === session.id)} class="session-card" oncontextmenu={(event) => openSessionMenu(event, session.id)}>
            <header class="card-header">
              <div class="session-identity"><span class="cli-badge">{session.cli ? session.cli.slice(0, 1).toUpperCase() : '$'}</span><strong title={session.title}>{session.title}</strong><span class="agent">{session.cli ? `${cliNames[session.cli]}${session.model ? ` · ${session.model}` : ''}` : 'shell'}</span>
                {#if session.cli}<span class="activity-chip" data-activity={activity} title={activityLabels[activity]}>{activity === 'working' ? 'Working' : activity === 'waiting' ? 'Needs you' : 'Ready'}</span>{/if}
                {#if memoryLabel}<span class="memory-chip" title="Resident memory used by this session's shell and its child processes">{memoryLabel}</span>{/if}
                <span title={session.endpoint}>⌁ {viewModel.endpointLabel(session.endpoint)}</span>
                <button class="folder folder-confirm" disabled={agentRunning} title={agentRunning ? 'Stop the running agent (■) before changing folders' : `${session.projectPath} — click to change folder (also renames the session)`} onclick={(event) => { event.stopPropagation(); void viewModel.changeFolder(session.id); }}>⌂ {session.projectPath}</button>
                <button class="upload" disabled={!recordProfile} title={recordProfile ? 'Notes and tasks for this project (stored on this device)' : 'Keep this session in Ungrouped (+) to attach notes and tasks'} aria-label={`Notes and tasks for ${session.title}${openTasks ? `, ${openTasks} open` : ''}`} onclick={(event) => { event.stopPropagation(); if (recordProfile) recordsTarget = { profileId: recordProfile.id, name: recordProfile.name }; }}><svg class="records-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M3 3.5l1.2 1.2L6.4 2.5M3 8.5l1.2 1.2L6.4 7.5M3 13.5l1.2 1.2 2.2-2.2M8.5 4h5M8.5 9h5M8.5 14h5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>{#if openTasks}<span class="task-badge">{openTasks > 99 ? '99+' : openTasks}</span>{/if}</button>
                <button class="upload" disabled={viewModel.uploadingFiles[session.id]} title={session.endpoint === 'local' ? 'Upload files — copies them into this session’s folder' : 'Upload files — sends them into this session’s folder over SSH'} onclick={(event) => { event.stopPropagation(); void viewModel.uploadFiles(session.id); }}>{viewModel.uploadingFiles[session.id] ? '⇪…' : '⇪'}</button>
                <span class="state" data-status={session.status}>{session.status}</span>
                {#if !agentRunning && session.status === 'running'}
                  <select class="agent-picker" aria-label="Run AI agent" value="" onclick={(event) => event.stopPropagation()} onchange={(event) => { const picked = event.currentTarget.value as CliKind | ''; event.currentTarget.value = ''; if (picked) void viewModel.launchAgent(session.id, picked); }}>
                    <option value="">Run AI…</option>{#each viewModel.availableAgents as cli}<option value={cli}>{cliNames[cli]}</option>{/each}
                  </select>
                {/if}</div>
              <div class="card-actions">
                {#if !poppedSessionId}<button title="New window" onclick={(event) => { event.stopPropagation(); void viewModel.popOut(session.id); }}>↗</button>{/if}
                {#if session.status === 'running'}
                  <button class="stop" title="Exit AI agent and return to shell" onclick={(event) => { event.stopPropagation(); void viewModel.exitAgent(session.id); }}>■</button>
                {:else}
                  <button class="resume" title="Resume this session" aria-label={`Resume ${session.title}`} onclick={(event) => { event.stopPropagation(); void viewModel.reconnectSession(session.id); }}>⟳ Resume</button>
                {/if}
                <button class="close" title="Close session" onclick={(event) => { event.stopPropagation(); void viewModel.closeSession(session.id); }}>×</button>
              </div>
            </header>
            <TerminalPane
              sessionId={session.id}
              fontSize={viewModel.fontSize}
              layoutRevision={terminalLayoutRevision}
              registerOutput={(id, sink) => viewModel.registerTerminal(id, sink)}
              onInput={(id, data) => void viewModel.writeTerminal(id, data)}
              onResize={(id, cols, rows) => void viewModel.resizeTerminal(id, cols, rows)}
              onInterrupt={(id) => void viewModel.interruptSession(id)}
              onFocus={(id) => viewModel.selectSession(id)}
              onCurrentDirectory={(id, path) => viewModel.updateCurrentDirectory(id, path)}
              onActivityScreen={(id, screen) => viewModel.observeAgentScreen(id, screen)}
            />
            {#if session.status !== 'running'}
              <div class="disconnected-banner" role="alert">
                <span>Disconnected — {session.endpoint === 'local' ? 'the shell exited unexpectedly.' : `the connection to ${viewModel.endpointLabel(session.endpoint)} was lost.`}</span>
                <button onclick={(event) => { event.stopPropagation(); void viewModel.reconnectSession(session.id); }}>Resume</button>
              </div>
            {/if}
          </article>
        {/each}
        {#if !visibleSessions.length}<div role="group" aria-label="Active group sessions" class="empty group-empty" ondragover={handleActiveGroupDragOver} ondrop={handleActiveGroupDrop}>No running sessions in this group.<small>Drop a session from the left to move it here.</small></div>{/if}
      </section>
    {:else}<div role="group" aria-label="Active group sessions" class="empty" ondragover={handleActiveGroupDragOver} ondrop={handleActiveGroupDrop}>No running sessions in this group.<small>Drop a session from the left to move it here.</small></div>{/if}
  </main>
</div>

{#if draggingSessionId}
  <div
    class="trash-zone"
    class:hover={trashHover}
    role="button"
    tabindex="-1"
    aria-label="Drop here to delete"
    ondragover={handleTrashDragOver}
    ondragleave={() => { trashHover = false; }}
    ondrop={handleTrashDrop}
  >🗑 Drop here to delete</div>
{/if}

{#if recordsTarget}
  <div class="settings-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) recordsTarget = null; }}>
    <div class="records-dialog" role="dialog" aria-modal="true" aria-label="Notes and tasks">
      {#key recordsTarget.profileId}
        <SessionRecords profileId={recordsTarget.profileId} sessionName={recordsTarget.name} repository={recordRepository} revision={viewModel.recordsRevision} syncNote={recordSyncNote} onChanged={() => { localRecordsRevision += 1; viewModel.requestSync(); }} onClose={() => { recordsTarget = null; }} />
      {/key}
    </div>
  </div>
{/if}

{#if sessionMenu}
  <div class="session-context-menu" style={`left:${sessionMenu.x}px;top:${sessionMenu.y}px`} role="menu" tabindex="-1">
    <button role="menuitem" onclick={renameFromMenu}>Rename…</button>
  </div>
{/if}

{#if groupMenu}
  <div class="session-context-menu" style={`left:${groupMenu.x}px;top:${groupMenu.y}px`} role="menu" tabindex="-1">
    <button role="menuitem" onclick={renameGroupFromMenu}>Rename…</button>
  </div>
{/if}

{#if groupRename}
  <div class="rename-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) groupRename = null; }}>
    <div class="rename-panel" role="dialog" aria-modal="true" aria-labelledby="group-rename-title">
      <form class="rename-form" onsubmit={(event) => { event.preventDefault(); saveGroupName(); }}>
        <strong id="group-rename-title">Rename group</strong>
        <input bind:value={groupRename.name} aria-label="Group name" required />
        <div class="rename-actions">
          <button type="button" onclick={() => { groupRename = null; }}>Cancel</button>
          <button class="primary" type="submit">Rename</button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if viewModel.settingsOpen}
  <div class="settings-backdrop" role="presentation" onclick={(event) => { if (event.target === event.currentTarget) viewModel.toggleSettings(); }}>
    <div class="settings-panel" role="dialog" aria-modal="true" aria-label="Settings">
      <header><div><strong>Settings</strong><small>Manage your account and AI agents.</small></div><button class="icon-button" aria-label="Close settings" onclick={() => viewModel.toggleSettings()}>×</button></header>
      <section class="account-settings" aria-label="Account">
        <div class="account-copy">
          <span class="account-avatar" class:signed-in={viewModel.authenticated}>{viewModel.authenticated ? viewModel.authUser?.name.slice(0, 1).toUpperCase() : 'G'}</span>
          <span>
            <strong>{viewModel.authUser?.name ?? 'Sign in to fastade'}</strong>
            <small>{viewModel.authUser?.email ?? 'Sync your session information across devices.'}</small>
          </span>
        </div>
        {#if viewModel.authenticated}
          <button type="button" class="account-action" disabled={viewModel.authBusy} onclick={() => void viewModel.signOut()}>{viewModel.authBusy ? 'Signing out…' : 'Sign out'}</button>
        {:else}
          <button type="button" class="google-login" disabled={!viewModel.googleLoginConfigured || viewModel.authBusy} title={viewModel.googleLoginConfigured ? 'Continue with Google' : 'Google OAuth is not configured'} onclick={() => void viewModel.signInWithGoogle()}>
            <span aria-hidden="true">G</span> {viewModel.authBusy ? 'Waiting for Google…' : 'Continue with Google'}
          </button>
        {/if}
        {#if viewModel.authError}<p class="account-note error-note">{viewModel.authError}</p>{:else if !viewModel.googleLoginConfigured}<p class="account-note">Add a Google Desktop OAuth client to enable login.</p>{/if}
      </section>
      <div class="settings-section-heading"><strong>AI agents</strong><small>Manage web chat and usage pages.</small></div>
      <div class="agent-settings-list">
        {#each viewModel.webAgents as agent (agent.id)}
          <div class="agent-setting-row">
            <span class="agent-color" style={`background:${agent.accent}`}></span><span><strong>{agent.name}</strong><small>{agent.chatUrl}</small></span>
            {#if agent.builtIn && agent.id in cliNames}
              {@const cli = agent.id as CliKind}
              <label class="agent-model-inline" title="Applied as --model when this CLI runs in a session. Type any model ID or choose one discovered from the CLI cache.">
                <input list={`agent-models-${cli}`} value={viewModel.agentDefaultModels[cli]} placeholder="CLI default" onchange={(event) => viewModel.setAgentDefaultModel(cli, event.currentTarget.value)} />
                <datalist id={`agent-models-${cli}`}>
                  {#each viewModel.knownModelAliases(cli) as alias}<option value={alias}></option>{/each}
                </datalist>
              </label>
              <button
                type="button"
                class="mcp-toggle"
                class:active={viewModel.mcpAgentEnabled[cli]}
                role="switch"
                aria-checked={viewModel.mcpAgentEnabled[cli]}
                title="Registers fastade's read-only session status tool and lifecycle hooks for activity and effective-model metadata."
                onclick={() => void viewModel.setMcpAgentEnabled(cli, !viewModel.mcpAgentEnabled[cli])}
              >
                <span class="toggle-track"><span class="toggle-thumb"></span></span>
                <span class="toggle-label">{viewModel.mcpAgentEnabled[cli] ? 'Uninstall MCP' : 'Install MCP'}</span>
              </button>
            {/if}
            <label class="agent-enabled"><input type="checkbox" checked={agent.enabled} onchange={(event) => viewModel.saveWebAgent({ ...agent, enabled: event.currentTarget.checked })} /><span>Show</span></label>
            <button onclick={() => editAgent(agent)}>Edit</button><button class="danger" onclick={() => viewModel.deleteWebAgent(agent.id)}>Delete</button>
          </div>
        {/each}
      </div>
      {#if agentEditorOpen}
        <form class="agent-editor" onsubmit={(event) => { event.preventDefault(); saveAgent(); }}>
          <p>{editingAgentId ? 'Edit agent' : 'New agent'}</p>
          <label>Name<input bind:value={agentDraft.name} placeholder="Perplexity" required /></label>
          <label>Color<input class="color-input" type="color" bind:value={agentDraft.accent} /></label>
          <label class="wide">Chat URL<input bind:value={agentDraft.chatUrl} type="url" placeholder="https://…" required /></label>
          <label class="wide">Usage URL<input bind:value={agentDraft.usageUrl} type="url" placeholder="Optional" /></label>
          <div class="editor-actions wide"><button type="button" onclick={closeAgentEditor}>Cancel</button><button class="primary" type="submit">OK</button></div>
        </form>
      {:else}
        <div class="add-agent-action"><button type="button" onclick={addAgent}><span aria-hidden="true">+</span> Add agent</button></div>
      {/if}
    </div>
  </div>
{/if}

{#if viewModel.remoteBrowser}
  {@const browser = viewModel.remoteBrowser}
  <RemoteFolderPicker
    endpoint={browser.endpoint}
    label={viewModel.endpointLabel(browser.endpoint)}
    initialPath={viewModel.remoteBrowserInitialPath}
    client={viewModel.client}
    onSelect={(path) => viewModel.applyRemoteFolderSelection(path)}
    onClose={() => viewModel.closeRemoteBrowser()}
  />
{/if}

{#if viewModel.serverPickerOpen}
  <ServerPicker
    sshHosts={viewModel.sshHosts}
    managedServers={viewModel.managedServers}
    generatedKey={viewModel.lastGeneratedKey}
    client={viewModel.client}
    onSelectSshHost={(alias) => void viewModel.startOnSshHost(alias)}
    onSelectManagedServer={(server) => void viewModel.startOnManagedServer(server)}
    onCreateServer={(input) => viewModel.createManagedServer(input)}
    onUpdateServer={(input) => viewModel.updateManagedServer(input)}
    onDeleteServer={(id) => void viewModel.deleteManagedServer(id)}
    onDismissGeneratedKey={() => viewModel.dismissGeneratedKey()}
    onClose={() => viewModel.closeServerPicker()}
  />
{/if}
