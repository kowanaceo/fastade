import type { SyncPushChange, SyncPushResponse, SyncSnapshot } from '../domain/session';
import type { SessionRecord } from '../domain/session-record';
import {
  activeRecordEntities, conflictCopy, planRecordChanges, recordFromEntity, syncMetaFrom,
  type RecordSyncMeta,
} from '../domain/session-record-sync';
import type { SessionRecordRepository } from './session-record-repository';

const STORAGE_KEY = 'fastade.record-sync.v1';

export interface RecordSyncTransport {
  syncSnapshot(): Promise<SyncSnapshot>;
  syncPush(changes: SyncPushChange[]): Promise<SyncPushResponse>;
}

export interface RecordSyncOutcome {
  /** Local text kept as copies because the record changed elsewhere. */
  conflicts: number;
  /** The cache was not replaced or copies were created; run again. */
  again: boolean;
}

function readMeta(userId: string): Record<string, RecordSyncMeta> {
  try {
    const users = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as Record<string, Record<string, RecordSyncMeta>>;
    return users[userId] ?? {};
  } catch {
    return {};
  }
}

function writeMeta(userId: string, meta: Record<string, RecordSyncMeta>): void {
  try {
    const users = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as Record<string, Record<string, RecordSyncMeta>>;
    users[userId] = meta;
    localStorage.setItem(STORAGE_KEY, JSON.stringify(users));
  } catch { /* the next sync recomputes it from the server snapshot */ }
}

const fingerprint = (records: SessionRecord[]): string =>
  JSON.stringify([...records].sort((a, b) => a.id.localeCompare(b.id)));

/** Two-way sync of notes and tasks with the account. The server snapshot is
 * authoritative: after pushing local edits the cache is replaced with it.
 * Failures throw before the cache is touched, so a failed sync never loses
 * local data. */
export async function syncSessionRecords(
  transport: RecordSyncTransport,
  store: SessionRecordRepository,
  options: { userId: string; mayMigrateLocal: boolean; backupLocal: boolean },
): Promise<RecordSyncOutcome> {
  const previous = readMeta(options.userId);
  let snapshot = await transport.syncSnapshot();
  const local = await store.listAll();
  const changes = planRecordChanges({
    previous, local, remote: activeRecordEntities(snapshot.entities), mayMigrateLocal: options.mayMigrateLocal,
  });

  const conflicted: SessionRecord[] = [];
  if (changes.length) {
    const response = await transport.syncPush(changes);
    const rejected = response.results.filter((result) => result.status === 'rejected');
    if (rejected.length) {
      throw new Error(rejected.map((result) => `${result.entityId}: ${result.reason ?? 'rejected'}`).join('; '));
    }
    const upserts = new Map(changes.filter((change) => change.operation === 'upsert').map((change) => [change.changeId, change.entityId]));
    for (const result of response.results) {
      const id = result.status === 'conflict' ? upserts.get(result.changeId) : undefined;
      const record = id ? local.find((item) => item.id === id) : undefined;
      if (record) conflicted.push(record);
    }
    snapshot = await transport.syncSnapshot();
  }

  // An edit made while the request was in flight must not be overwritten by
  // the snapshot taken before it; try again once the cache is quiet.
  if (fingerprint(await store.listAll()) !== fingerprint(local)) return { conflicts: 0, again: true };

  const remote = activeRecordEntities(snapshot.entities);
  const records = [...remote.values()].flatMap((entity) => recordFromEntity(entity) ?? []);
  const now = Date.now();
  const copies = conflicted.map((record) => conflictCopy(record, crypto.randomUUID(), now));
  await store.replaceAll([...records, ...copies], options.backupLocal);
  writeMeta(options.userId, syncMetaFrom(remote));
  return { conflicts: copies.length, again: copies.length > 0 };
}
