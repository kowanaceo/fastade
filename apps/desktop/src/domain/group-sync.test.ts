import assert from 'node:assert/strict';
import test from 'node:test';
import type { SyncEntity } from './session.ts';
import {
  activeGroupEntities, groupHash, groupsFromEntities, groupSyncStateFrom, planGroupChanges,
  profileGroupFromPayload, profileGroupPayload,
} from './group-sync.ts';

const ids = () => { let n = 0; return () => `c${++n}`; };
const entity = (entityId: string, payload: Record<string, unknown>, version = 1, deleted = false): SyncEntity =>
  ({ entityType: 'group', entityId, version, payload, deleted, updatedAt: '2026-10-04T00:00:00Z' });
const defaultEntity = (name: string, version = 1): SyncEntity =>
  ({ entityType: 'setting', entityId: 'defaultGroup', version, payload: { name }, deleted: false, updatedAt: '2026-10-04T00:00:00Z' });
const plan = (over: Partial<Parameters<typeof planGroupChanges>[0]>) => planGroupChanges({
  previous: { groups: {} }, local: [], defaultName: 'Sessions', builtinDefaultName: 'Sessions',
  remote: { groups: new Map() }, mayMigrateLocal: true, now: 10, newChangeId: ids(), ...over,
});

test('groups are ordered by server order and bad rows are skipped', () => {
  const groups = new Map([
    ['b', entity('b', { name: 'B', order: 1, createdAt: 1 })],
    ['a', entity('a', { name: 'A', order: 0, createdAt: 2 })],
    ['x', entity('x', { name: '  ', order: 2 })],
    ['f', entity('f', { name: 'F', order: 'a0', createdAt: 3 })],
  ]);
  assert.deepEqual(groupsFromEntities(groups).map((group) => group.id), ['group-a', 'group-b', 'group-f']);
});

test('deleted entities are ignored', () => {
  const found = activeGroupEntities([entity('a', { name: 'A', order: 0 }, 2, true), defaultEntity('Home')]);
  assert.equal(found.groups.size, 0);
  assert.equal(found.defaultGroup?.payload.name, 'Home');
});

test('first sync uploads local groups in order only when migration is allowed', () => {
  const local = [{ id: 'group-a', name: 'A', createdAt: 1 }, { id: 'group-b', name: 'B' }];
  const changes = plan({ local });
  assert.deepEqual(changes.map((change) => [change.entityType, change.entityId, change.payload.order]), [['group', 'a', 0], ['group', 'b', 1]]);
  assert.equal(changes[0].payload.createdAt, 1);
  assert.equal(changes[1].payload.createdAt, 10);
  assert.equal(changes[0].payload.updatedAt, 10);
  assert.equal(plan({ local, mayMigrateLocal: false }).length, 0);
});

test('rename, move and delete are pushed against the synced version', () => {
  const previous = { groups: { a: { version: 3, hash: groupHash('A', 0) }, b: { version: 2, hash: groupHash('B', 1) }, gone: { version: 5, hash: groupHash('G', 2) } } };
  const changes = plan({ previous, local: [{ id: 'group-b', name: 'B' }, { id: 'group-a', name: 'A2' }] });
  const summary = changes.map((change) => [change.entityId, change.operation, change.baseVersion]);
  assert.deepEqual(summary, [['b', 'upsert', 2], ['a', 'upsert', 3], ['gone', 'delete', 5]]);
  assert.equal(plan({ previous: { groups: { a: previous.groups.a } }, local: [{ id: 'group-a', name: 'A' }] }).length, 0);
});

test('renamed default group is a setting; the built-in name is not uploaded', () => {
  assert.equal(plan({}).length, 0);
  const [change] = plan({ defaultName: 'Home' });
  assert.deepEqual([change.entityType, change.entityId, change.payload], ['setting', 'defaultGroup', { name: 'Home' }]);
  const synced = plan({ previous: { groups: {}, defaultGroup: { version: 4, hash: JSON.stringify(['Home']) } }, defaultName: 'Work' });
  assert.equal(synced[0].baseVersion, 4);
});

test('state taken from a snapshot makes the same local list clean', () => {
  const remote = activeGroupEntities([
    entity('b', { name: 'B', order: 5, createdAt: 1 }, 7), entity('a', { name: 'A', order: 2, createdAt: 1 }, 4), defaultEntity('Home', 3),
  ]);
  const local = groupsFromEntities(remote.groups);
  assert.equal(plan({ previous: groupSyncStateFrom(remote), local, defaultName: 'Home', remote }).length, 0);
});

test('profile membership maps to entity ids and falls back to the default group', () => {
  const groups = [{ id: 'group-a', name: 'A' }];
  assert.equal(profileGroupPayload('group-a', groups), 'a');
  assert.equal(profileGroupPayload('group-zz', groups), null);
  assert.equal(profileGroupPayload(undefined, groups), null);
  assert.equal(profileGroupFromPayload('a', groups), 'group-a');
  assert.equal(profileGroupFromPayload('deleted', groups), undefined);
  assert.equal(profileGroupFromPayload(null, groups), undefined);
});
