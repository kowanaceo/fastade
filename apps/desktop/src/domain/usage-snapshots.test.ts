import assert from 'node:assert/strict';
import test from 'node:test';
import type { AgentUsage, UsageSnapshot } from './session.ts';
import { localHostId, localSnapshot, mergeHostUsage, parseSnapshotList, shouldUpload, snapshotAge, unlistedAgentIds, snapshotKey, windowsKey } from './usage-snapshots.ts';

const windows = [{ label: '5h', remainingPercent: 72, resetsAt: 100 }];
const snap = (host: string, at: number, agentId = 'codex'): UsageSnapshot => ({ agentId, hostId: host, hostLabel: host, collectedAt: at, windows });

test('host ids are safe for a url segment', () => {
  assert.equal(localHostId('a b/c:1'), 'device-a-b-c-1');
  assert.equal(localHostId(null), 'device-local');
});

test('only an available result with windows becomes a local snapshot', () => {
  const usage: AgentUsage = { agentId: 'codex', status: 'available', windows, collectedAt: 5 };
  assert.equal(localSnapshot('codex', usage, 'd1')?.hostId, 'device-d1');
  assert.equal(localSnapshot('codex', { ...usage, status: 'unavailable' }, 'd1'), null);
  assert.equal(localSnapshot('codex', { ...usage, windows: [] }, 'd1'), null);
  assert.equal(localSnapshot('codex', undefined, 'd1'), null);
  for (const collectedAt of [undefined, NaN, 0, 5.1]) {
    assert.equal(localSnapshot('codex', { ...usage, collectedAt }, 'd1'), null);
  }
});

test('upload when numbers change or the last upload is old, not otherwise', () => {
  const s = snap('h', 1000);
  const same = { key: snapshotKey(s), at: 1000, collectedAt: 1000 };
  assert.equal(shouldUpload(undefined, s, 1000), true);
  assert.equal(shouldUpload(same, s, 1000 + 60), false);
  assert.equal(shouldUpload(same, s, 1000 + 20 * 60), true);
  assert.equal(shouldUpload({ key: 'other', at: 1000, collectedAt: 1000 }, s, 1001), true);
  assert.equal(shouldUpload(same, { ...s, collectedAt: 999 }, 3000), false);
});

test('cached snapshots preserve collection time, provider account id and expired resets on every host', () => {
  const usage: AgentUsage = { agentId: 'codex', status: 'available', collectedAt: 1000, accountId: 'provider-account', windows };
  for (const host of ['device-a', 'device-b']) {
    const snapshot = localSnapshot('codex', usage, host)!;
    assert.equal(snapshot.collectedAt, 1000);
    assert.equal(snapshot.accountId, 'provider-account');
    assert.equal(snapshot.windows[0].resetsAt, 100);
  }
});

test('wire windows prefer epoch seconds and omit null reset fields while preserving fallback text', () => {
  const usage = { agentId: 'claude', status: 'available', collectedAt: 1000, windows: [
    { label: '5h', remainingPercent: 10, resetsAt: 100, resetText: null },
    { label: 'Week', remainingPercent: 20, resetsAt: null, resetText: 'Resets after the next billing cycle' },
    { label: 'Sonnet', remainingPercent: 30, resetsAt: null, resetText: null },
  ] } as unknown as AgentUsage;
  assert.deepEqual(JSON.parse(JSON.stringify(localSnapshot('claude', usage, 'd'))).windows, [
    { label: '5h', remainingPercent: 10, resetsAt: 100 },
    { label: 'Week', remainingPercent: 20, resetText: 'Resets after the next billing cycle' },
    { label: 'Sonnet', remainingPercent: 30 },
  ]);
});

test('changed fallback reset text triggers an upload even when percentage is unchanged', () => {
  const first = [{ label: 'Week', remainingPercent: 70, resetText: 'Resets on a holiday' }];
  const snapshot = { ...snap('h', 1001), windows: [{ ...first[0], resetText: 'Resets next business day' }] };
  assert.equal(shouldUpload({ key: snapshotKey({ ...snapshot, windows: first }), at: 1000, collectedAt: 1000 }, snapshot, 1001), true);
});

test('server lists are parsed defensively', () => {
  assert.equal(parseSnapshotList(null).length, 0);
  assert.equal(parseSnapshotList({ snapshots: [snap('a', 1)] }).length, 1);
  assert.equal(parseSnapshotList([snap('a', 1), { agentId: 'x' }, 'junk']).length, 1);
  assert.equal(parseSnapshotList([{ ...snap('a', 1), hostLabel: '' }])[0].hostLabel, 'a');
});

test('merge keeps the newest per agent and host and drops this device', () => {
  const merged = mergeHostUsage([[snap('own', 9), snap('b', 1), snap('a', 2)], [snap('b', 5), snap('b', 3, 'claude')]], 'own');
  assert.deepEqual(merged.map((s) => [s.agentId, s.hostId, s.collectedAt]), [['codex', 'a', 2], ['codex', 'b', 5], ['claude', 'b', 3]]);
  assert.equal(mergeHostUsage([[{ ...snap('x', 1), windows: [] }]], 'own').length, 0);
});

test('age labels and staleness', () => {
  assert.deepEqual(snapshotAge(1000, 1030), { label: 'now', stale: false });
  assert.deepEqual(snapshotAge(1000, 1000 + 600), { label: '10m ago', stale: false });
  assert.deepEqual(snapshotAge(1000, 1000 + 7200), { label: '2h ago', stale: true });
  assert.deepEqual(snapshotAge(1000, 1000 + 3 * 86_400), { label: '3d ago', stale: true });
});

test('snapshots of agents without a row are still listed', () => {
  const all = [snap('a', 1), snap('a', 2, 'claude'), snap('b', 3, 'aider'), snap('c', 4, 'aider')];
  assert.deepEqual(unlistedAgentIds(all, ['codex']), ['aider', 'claude']);
  assert.deepEqual(unlistedAgentIds(all, ['codex', 'claude', 'aider']), []);
});

test('hosts on one account collapse into a single entry, and this device\'s account is dropped', () => {
  const acct = (host: string, at: number, accountId?: string): UsageSnapshot => ({ ...snap(host, at), accountId });
  const merged = mergeHostUsage([[acct('backend', 5, 'A'), acct('dev', 9, 'A'), acct('other', 3, 'B'), acct('mine', 4, 'ME')]], 'own', [{ agentId: 'codex', accountId: 'ME' }]);
  assert.deepEqual(merged.map((s) => [s.hostLabel, s.collectedAt]), [['backend, dev', 9], ['other', 3]]);
});

test('snapshot upload binds email to its collected account and omits unknown identity', () => {
  const usage: AgentUsage = { agentId: 'codex', status: 'available', collectedAt: 1000, accountId: 'account-a', accountEmail: 'a@example.com', windows };
  const payload = JSON.parse(JSON.stringify(localSnapshot('codex', usage, 'test')));
  assert.equal(payload.accountId, 'account-a');
  assert.equal(payload.accountEmail, 'a@example.com');
  assert.equal(payload.collectedAt, 1000);
  assert.equal('accountEmail' in JSON.parse(JSON.stringify(localSnapshot('codex', { ...usage, accountEmail: undefined }, 'test'))), false);
  assert.equal('accountEmail' in JSON.parse(JSON.stringify(localSnapshot('codex', { ...usage, accountId: undefined }, 'test'))), false);
  assert.equal(parseSnapshotList({ snapshots: [payload] })[0].accountEmail, 'a@example.com');
  assert.equal(parseSnapshotList([{ ...payload, accountEmail: 123 }])[0].accountEmail, undefined);
});


test('email-only change uploads while an older sample remains blocked', () => {
  const original = { ...snap('h', 1000), accountId: 'account-a', accountEmail: 'first@example.com' };
  const previous = { key: snapshotKey(original), at: 1000, collectedAt: 1000 };
  assert.equal(shouldUpload(previous, { ...original, accountEmail: 'second@example.com' }, 1001), true);
  assert.equal(shouldUpload(previous, { ...original, collectedAt: 999, accountEmail: 'second@example.com' }, 1001), false);
});
