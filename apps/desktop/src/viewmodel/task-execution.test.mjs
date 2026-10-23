import assert from 'node:assert/strict';
import { before, after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

let server;
let AppViewModel;
const originalStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
before(async () => {
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true, value: { getItem: () => null, setItem: () => {} },
  });
  server = await createServer({
    root: fileURLToPath(new URL('../../../../', import.meta.url)),
    configFile: false, plugins: [svelte()], logLevel: 'error',
    server: { middlewareMode: true, hmr: false, ws: false },
    optimizeDeps: { noDiscovery: true }, appType: 'custom',
  });
  ({ AppViewModel } = await server.ssrLoadModule('/apps/desktop/src/viewmodel/app-view-model.svelte.ts'));
});
after(async () => {
  await server?.close();
  if (originalStorage) Object.defineProperty(globalThis, 'localStorage', originalStorage);
  else delete globalThis.localStorage;
});

function fixture() {
  const session = { id: 'runtime-1', title: 'Project', endpoint: 'local', projectPath: '/project', status: 'running' };
  const writes = [];
  const claims = [];
  const restored = [];
  const records = [{ id: 'task-1', kind: 'task', status: 'in progress', title: 'Implement feature', content: 'Full requirements', profileId: 'profile-1', createdAt: 1, updatedAt: 2 }];
  const client = {
    writeTerminal: async (id, data) => { writes.push({ id, data }); },
    updateSessionAgent: async (_id, cli, model) => ({ ...session, cli, model }),
    clearSessionActivityOverride: async () => {},
    refreshSessionAgentMetadata: async () => session,
  };
  const store = {
    claimTodoTasks: async (profileId, ids) => { claims.push({ profileId, ids }); return records; },
    restoreClaimedTasks: async (values) => { restored.push(values); },
  };
  const vm = new AppViewModel(client, store);
  vm.sessions = [session];
  vm.savedSessions = [{ id: 'profile-1', name: 'Project', lastEndpoint: 'local', devicePaths: { local: '/project' } }];
  vm.sessionProfileIds = { 'runtime-1': 'profile-1' };
  return { vm, client, store, session, writes, claims, restored, records };
}

test('play sends one submitted request to the pinned running agent', async () => {
  const f = fixture();
  f.vm.runningAgents = { 'runtime-1': 'claude' };
  f.vm.sessions.push({ ...f.session, id: 'other', projectPath: '/elsewhere' });
  f.vm.selectedSessionId = 'other';
  await f.vm.runTasks('runtime-1', 'profile-1', ['task-1']);
  assert.deepEqual(f.claims, [{ profileId: 'profile-1', ids: ['task-1'] }]);
  assert.equal(f.writes.length, 1);
  assert.equal(f.writes[0].id, 'runtime-1');
  assert.match(f.writes[0].data, /^Execute the following saved tasks/);
  assert.ok(f.writes[0].data.endsWith('\r'));
  assert.equal(f.vm.selectedSessionId, 'runtime-1');
  assert.equal(f.vm.activityFor('runtime-1'), 'working');
});

test('a bare shell starts the most recent enabled CLI with its configured model and initial prompt', async () => {
  const f = fixture();
  f.vm.sessions[0].lastModels = { claude: { detectedAt: '2026-10-19T00:00:00Z' } };
  f.vm.agentDefaultModels.claude = 'preferred-model';
  await f.vm.runTasks('runtime-1', 'profile-1');
  assert.equal(f.claims[0].ids, undefined);
  assert.equal(f.writes.length, 1);
  assert.match(f.writes[0].data, /^claude --model 'preferred-model' 'Execute /);
  assert.equal(f.vm.runningAgents['runtime-1'], 'claude');
});

test('a bare shell without history starts the first enabled AI', async () => {
  const f = fixture();
  await f.vm.runTasks('runtime-1', 'profile-1');
  assert.match(f.writes[0].data, /^codex 'Execute /);
});

test('busy and approval-waiting agents cannot receive task input as an approval response', async () => {
  for (const activity of ['working', 'waiting']) {
    const f = fixture();
    f.vm.runningAgents = { 'runtime-1': 'codex' };
    f.vm.agentActivity = { 'runtime-1': activity };
    assert.match(f.vm.taskRunUnavailable('runtime-1'), /Ready/);
    await assert.rejects(f.vm.runTasks('runtime-1', 'profile-1'), /Ready/);
    assert.equal(f.claims.length, 0);
    assert.equal(f.writes.length, 0);
  }
});

test('disconnected sessions and unrelated profiles never claim or send tasks', async () => {
  const f = fixture();
  f.vm.sessions[0].status = 'failed';
  await assert.rejects(f.vm.runTasks('runtime-1', 'profile-1'), /Reconnect/);
  f.vm.sessions[0].status = 'running';
  await assert.rejects(f.vm.runTasks('runtime-1', 'other-profile'), /no longer matches/);
  assert.equal(f.claims.length, 0);
  assert.equal(f.writes.length, 0);
});

test('a disconnection during claim restores the claim without sending input', async () => {
  const f = fixture();
  f.store.claimTodoTasks = async () => {
    f.vm.sessions[0].status = 'failed';
    return f.records;
  };
  await assert.rejects(f.vm.runTasks('runtime-1', 'profile-1'), /Reconnect/);
  assert.deepEqual(f.restored, [f.records]);
  assert.equal(f.writes.length, 0);
});

test('duplicate clicks cannot start another handoff while claiming', async () => {
  const f = fixture();
  let finishClaim;
  f.store.claimTodoTasks = () => new Promise((resolve) => { finishClaim = resolve; });
  const first = f.vm.runTasks('runtime-1', 'profile-1');
  await assert.rejects(f.vm.runTasks('runtime-1', 'profile-1'), /already being sent/);
  finishClaim(f.records);
  await first;
  assert.equal(f.writes.length, 1);
});

test('terminal write errors reach the dialog and restore task status', async () => {
  const f = fixture();
  f.vm.runningAgents = { 'runtime-1': 'claude' };
  f.client.writeTerminal = async () => { throw new Error('PTY disconnected'); };
  await assert.rejects(f.vm.runTasks('runtime-1', 'profile-1'), /Could not send tasks/);
  assert.deepEqual(f.restored, [f.records]);
});
