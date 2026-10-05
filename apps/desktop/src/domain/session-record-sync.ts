import type { SyncEntity, SyncPushChange } from './session.ts';
import { validateRecordDraft, type SessionRecord } from './session-record.ts';

/** The server is the source of truth while signed in. Each record's last
 * synced server version and content hash are remembered so a local edit can be
 * told apart from a newer remote version. */
export interface RecordSyncMeta { version: number; hash: string; }

export const RECORD_ENTITY_TYPE = 'session_record';

export function recordPayload(record: SessionRecord): Record<string, unknown> {
  return {
    profileId: record.profileId,
    kind: record.kind,
    title: record.title,
    content: record.content,
    ...(record.kind === 'task' ? { status: record.status } : {}),
    createdAt: record.createdAt,
    updatedAt: record.updatedAt,
  };
}

/** Timestamps are excluded: only a change the user made should count as dirty. */
export function recordHash(record: SessionRecord): string {
  return JSON.stringify([
    record.profileId, record.kind, record.title, record.content,
    record.kind === 'task' ? record.status : null,
  ]);
}

/** Returns null for entities this client cannot understand, so a bad remote
 * row is skipped instead of breaking the whole sync. */
export function recordFromEntity(entity: SyncEntity): SessionRecord | null {
  const payload = entity.payload;
  if (typeof payload.profileId !== 'string' || !payload.profileId) return null;
  try {
    const draft = validateRecordDraft({
      kind: payload.kind, title: payload.title, content: payload.content, status: payload.status,
    });
    const fallback = Date.parse(entity.updatedAt) || 0;
    return {
      ...draft,
      id: entity.entityId,
      profileId: payload.profileId,
      createdAt: typeof payload.createdAt === 'number' ? payload.createdAt : fallback,
      updatedAt: typeof payload.updatedAt === 'number' ? payload.updatedAt : fallback,
    };
  } catch {
    return null;
  }
}

export function activeRecordEntities(entities: SyncEntity[]): Map<string, SyncEntity> {
  const records = new Map<string, SyncEntity>();
  for (const entity of entities) {
    if (entity.entityType === RECORD_ENTITY_TYPE && !entity.deleted) records.set(entity.entityId, entity);
  }
  return records;
}

/** Local edits to push. A record never seen by the server is uploaded only
 * when the cache is known to belong to this account (`mayMigrateLocal`), and
 * a record the server knew but the user deleted is pushed as a delete. */
export function planRecordChanges(input: {
  previous: Record<string, RecordSyncMeta>;
  local: SessionRecord[];
  remote: Map<string, SyncEntity>;
  mayMigrateLocal: boolean;
  newChangeId?: () => string;
}): SyncPushChange[] {
  const newChangeId = input.newChangeId ?? (() => crypto.randomUUID());
  const changes: SyncPushChange[] = [];
  const localIds = new Set<string>();
  for (const record of input.local) {
    localIds.add(record.id);
    const known = input.previous[record.id];
    const dirty = known
      ? known.hash !== recordHash(record)
      : !input.remote.has(record.id) && input.mayMigrateLocal;
    if (!dirty) continue;
    changes.push({
      changeId: newChangeId(),
      entityType: RECORD_ENTITY_TYPE,
      entityId: record.id,
      operation: 'upsert',
      ...(known ? { baseVersion: known.version } : {}),
      payload: recordPayload(record),
    });
  }
  for (const [id, known] of Object.entries(input.previous)) {
    if (localIds.has(id)) continue;
    changes.push({
      changeId: newChangeId(),
      entityType: RECORD_ENTITY_TYPE,
      entityId: id,
      operation: 'delete',
      baseVersion: known.version,
      payload: {},
    });
  }
  return changes;
}

/** A local edit the server refused because the record changed elsewhere is
 * kept as a separate copy instead of being overwritten by the server version. */
export function conflictCopy(record: SessionRecord, id: string, now: number): SessionRecord {
  return { ...record, id, title: `${record.title} (conflict copy)`, createdAt: now, updatedAt: now };
}

export function syncMetaFrom(entities: Map<string, SyncEntity>): Record<string, RecordSyncMeta> {
  const meta: Record<string, RecordSyncMeta> = {};
  for (const [id, entity] of entities) {
    const record = recordFromEntity(entity);
    if (record) meta[id] = { version: entity.version, hash: recordHash(record) };
  }
  return meta;
}
