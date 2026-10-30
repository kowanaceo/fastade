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
  const sessions = new Map();
  let sequence = 0;
  const vm = new AppViewModel({
    createSession: async (input) => {
      const session = { ...input, id: `runtime-${++sequence}`, status: 'running' };
      sessions.set(session.id, session);
      return session;
    },
    listSessions: async () => [...sessions.values()],
    closeSession: async (id) => { sessions.delete(id); },
    setGroupLayout: async () => {},
    getRemoteUsage: async () => [],
  }, {});
  vm.groupDefinitions = [{ id: 'services', name: 'Services', createdAt: 1 }];
  vm.savedSessions = [{
    id: 'saved-server', name: 'kowanasdev', lastEndpoint: 'kowanasdev',
    devicePaths: { kowanasdev: '/workspace/live/projects/travels' },
  }];
  vm.profileGroupIds = { 'saved-server': 'services' };
  return vm;
}

test('saved entry keeps its name across connection, disconnection and reconnection', async () => {
  const vm = fixture();
  const entries = () => vm.sessionGroups.find((group) => group.id === 'services').entries;
  assert.equal(entries()[0].title, 'kowanasdev');
  await vm.startSaved(vm.savedSessions[0], 'services');
  assert.equal(vm.error, null);
  assert.equal(entries().length, 1);
  assert.equal(entries()[0].title, 'kowanasdev');
  assert.equal(vm.sessions[0].projectPath, '/workspace/live/projects/travels');
  await vm.closeSession(vm.sessions[0].id);
  assert.equal(vm.error, null);
  assert.equal(entries()[0].title, 'kowanasdev');
  await vm.startSaved(vm.savedSessions[0], 'services');
  assert.equal(vm.error, null);
  assert.equal(entries()[0].title, 'kowanasdev');
});

test('a new unsaved connection still takes its name from the folder', async () => {
  const vm = fixture();
  vm.savedSessions = [];
  await vm.createSession('ungrouped', 'kowanasdev', '/workspace/live/projects/travels');
  assert.equal(vm.error, null);
  assert.equal(vm.sessions[0].title, 'travels');
});
