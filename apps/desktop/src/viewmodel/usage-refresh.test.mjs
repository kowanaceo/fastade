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

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
const usage = (agentId, percent) => ({ agentId, status: 'available', windows: [{ label: '5h', remainingPercent: percent }] });
const tick = () => new Promise((resolve) => setImmediate(resolve));
function fixture(getAgentUsage) {
  return new AppViewModel({ getAgentUsage, getRemoteUsage: async () => [], getLocalAccounts: async () => [] }, {});
}

test('Codex renders while Claude is still pending and refresh state clears afterward', async () => {
  const claude = deferred();
  const calls = [];
  const vm = fixture(async (id, force) => {
    calls.push([id, force]);
    return id === 'codex' ? usage(id, 42) : claude.promise;
  });
  const refresh = vm.refreshAgentUsage(true);
  assert.equal(vm.usageRefreshing, true);
  await tick();
  assert.equal(vm.agentUsage.codex.windows[0].remainingPercent, 42);
  assert.equal(vm.agentUsage.claude, undefined);
  assert.deepEqual(calls, [['codex', true], ['claude', true]]);
  claude.resolve(usage('claude', 75));
  await refresh;
  assert.equal(vm.agentUsage.claude.windows[0].remainingPercent, 75);
  assert.equal(vm.usageRefreshing, false);
});

test('an older refresh cannot overwrite a newer manual refresh', async () => {
  const older = deferred();
  const vm = fixture(async (id, force) => force ? usage(id, 80) : older.promise);
  const first = vm.refreshAgentUsage();
  await vm.refreshAgentUsage(true);
  older.resolve(usage('codex', 10));
  await first;
  assert.equal(vm.agentUsage.codex.windows[0].remainingPercent, 80);
  assert.equal(vm.agentUsage.claude.windows[0].remainingPercent, 80);
});

test('timer ticks do not launch duplicate collectors during a pending refresh', async () => {
  const pending = deferred();
  let calls = 0;
  const vm = fixture(async (id) => { calls += 1; await pending.promise; return usage(id, 80); });
  const first = vm.refreshAgentUsage();
  await vm.refreshAgentUsage();
  assert.equal(calls, 2);
  pending.resolve();
  await first;
});

test('collector errors are displayed and leave refresh available again', async () => {
  const vm = fixture(async () => { throw new Error('lookup timed out'); });
  await vm.refreshAgentUsage(true);
  assert.equal(vm.agentUsage.codex.status, 'error');
  assert.equal(vm.agentUsage.codex.message, 'lookup timed out');
  assert.equal(vm.usageRefreshing, false);
});

test('uploads use each collector timestamp and bound account even after a slow peer or account change', async () => {
  const claude = deferred();
  const uploads = [];
  const codex = { ...usage('codex', 42), collectedAt: 1000, accountId: 'original-account', accountEmail: 'original@example.com' };
  const vm = new AppViewModel({
    getAgentUsage: async (id) => id === 'codex' ? codex : claude.promise,
    getRemoteUsage: async () => [],
    getLocalAccounts: async () => [{ agentId: 'codex', accountId: 'new-account', label: 'later@example.com' }],
    putUsageSnapshot: async (snapshot) => uploads.push(JSON.parse(JSON.stringify(snapshot))),
    listUsageSnapshots: async () => [],
  }, {});
  vm.authUser = { id: '1' };
  vm.authDeviceId = 'test';
  const refresh = vm.refreshAgentUsage(true);
  await tick();
  claude.resolve({ ...usage('claude', 70), collectedAt: 2000, accountId: 'claude-account' });
  await refresh;
  assert.equal(uploads.find((s) => s.agentId === 'codex').collectedAt, 1000);
  assert.equal(uploads.find((s) => s.agentId === 'codex').accountId, 'original-account');
  assert.equal(uploads.find((s) => s.agentId === 'codex').accountEmail, 'original@example.com');
  assert.equal(uploads.find((s) => s.agentId === 'claude').collectedAt, 2000);
  await vm.refreshAgentUsage(true);
  assert.equal(uploads.length, 2, 'a cached sample is not unnecessarily uploaded');
  codex.accountId = 'new-account';
  codex.collectedAt = 3000;
  await vm.refreshAgentUsage(true);
  assert.equal(uploads.length, 3, 'another account with identical limits must be uploaded');
  assert.equal(uploads[2].accountId, 'new-account');
  codex.accountEmail = 'changed@example.com';
  await vm.refreshAgentUsage(true);
  assert.equal(uploads.length, 4, 'an email change must upload even with identical usage');
  assert.equal(uploads[3].accountEmail, 'changed@example.com');
});
