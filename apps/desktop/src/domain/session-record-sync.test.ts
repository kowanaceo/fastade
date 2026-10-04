import assert from 'node:assert/strict';
import test from 'node:test';
import type { SyncEntity } from './session.ts';
import type { SessionRecord } from './session-record.ts';
import { conflictCopy, planRecordChanges, recordFromEntity, recordHash, recordPayload, syncMetaFrom } from './session-record-sync.ts';

const note: SessionRecord = { id: 'n1', profileId: 'p1', kind: 'note', title: 'Design', content: '  a\n', createdAt: 1, updatedAt: 2 };
const task: SessionRecord = { id: 't1', profileId: 'p1', kind: 'task', title: 'Ship', content: '', status: 'todo', createdAt: 1, updatedAt: 2 };
const ids = () => { let n = 0; return () => `c${++n}`; };

function entity(record: SessionRecord, version = 1): SyncEntity {
  return { entityType: 'session_record', entityId: record.id, version, payload: recordPayload(record), deleted: false, updatedAt: '2026-10-04T00:00:00Z' };
}

test('payload omits status for notes and round-trips through an entity', () => {
  assert.equal('status' in recordPayload(note), false);
  assert.deepEqual(recordFromEntity(entity(note)), note);
  assert.deepEqual(recordFromEntity(entity(task)), task);
});

test('malformed remote entities are skipped', () => {
  const bad = { ...entity(note), payload: { ...recordPayload(note), kind: 'memo' } };
  assert.equal(recordFromEntity(bad), null);
  assert.equal(recordFromEntity({ ...entity(note), payload: { kind: 'note', title: 'x', content: '' } }), null);
});

test('first sync uploads unknown local records only when migration is allowed', () => {
  const args = { previous: {}, local: [note], remote: new Map(), newChangeId: ids() };
  const up = planRecordChanges({ ...args, mayMigrateLocal: true });
  assert.equal(up.length, 1);
  assert.equal(up[0].operation, 'upsert');
  assert.equal('baseVersion' in up[0], false);
  assert.equal(planRecordChanges({ ...args, mayMigrateLocal: false }).length, 0);
});

test('a record the server already has is not re-uploaded', () => {
  const remote = new Map([[note.id, entity(note)]]);
  assert.equal(planRecordChanges({ previous: {}, local: [note], remote, mayMigrateLocal: true }).length, 0);
});

test('only real edits are pushed, with the last synced version as base', () => {
  const previous = { [note.id]: { version: 3, hash: recordHash(note) } };
  assert.equal(planRecordChanges({ previous, local: [{ ...note, updatedAt: 99 }], remote: new Map(), mayMigrateLocal: true }).length, 0);
  const edited = planRecordChanges({ previous, local: [{ ...note, content: 'new' }], remote: new Map(), mayMigrateLocal: true, newChangeId: ids() });
  assert.equal(edited.length, 1);
  assert.equal(edited[0].baseVersion, 3);
});

test('a status change on a task counts as an edit', () => {
  const previous = { [task.id]: { version: 1, hash: recordHash(task) } };
  const changes = planRecordChanges({ previous, local: [{ ...task, status: 'done' }], remote: new Map(), mayMigrateLocal: true });
  assert.equal(changes.length, 1);
});

test('a locally deleted record is pushed as a delete', () => {
  const previous = { [note.id]: { version: 4, hash: recordHash(note) } };
  const changes = planRecordChanges({ previous, local: [], remote: new Map(), mayMigrateLocal: true, newChangeId: ids() });
  assert.deepEqual(changes.map((c) => [c.operation, c.entityId, c.baseVersion]), [['delete', 'n1', 4]]);
});

test('conflict copy keeps the local text under a new id', () => {
  const copy = conflictCopy({ ...note, content: 'mine' }, 'x', 50);
  assert.equal(copy.id, 'x');
  assert.equal(copy.content, 'mine');
  assert.equal(copy.title, 'Design (conflict copy)');
});

test('sync metadata is derived from remote entities', () => {
  const meta = syncMetaFrom(new Map([[note.id, entity(note, 7)]]));
  assert.deepEqual(meta, { n1: { version: 7, hash: recordHash(note) } });
});
