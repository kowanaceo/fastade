import type { SyncEntity, SyncPushChange } from './session.ts';

/** Last synced server version and content hash, so a local edit can be told
 * apart from a newer remote version (same scheme as profiles and records). */
export interface GroupSyncMeta { version: number; hash: string; }

export interface GroupSyncState {
  groups: Record<string, GroupSyncMeta>;
  defaultGroup?: GroupSyncMeta;
}

export interface SyncGroup { id: string; name: string; createdAt?: number; }

export const GROUP_ENTITY_TYPE = 'group';
export const SETTING_ENTITY_TYPE = 'setting';
export const DEFAULT_GROUP_SETTING_ID = 'defaultGroup';

const LOCAL_ID_PREFIX = 'group-';

/** Local group ids are `group-<uuid>`; the server entity id is the bare uuid. */
export const groupEntityId = (localId: string): string =>
  localId.startsWith(LOCAL_ID_PREFIX) ? localId.slice(LOCAL_ID_PREFIX.length) : localId;

export const localGroupId = (entityId: string): string => `${LOCAL_ID_PREFIX}${entityId}`;

/** Timestamps are excluded: only a rename or a move counts as an edit. */
export const groupHash = (name: string, order: number | string): string => JSON.stringify([name, order]);

export const defaultGroupHash = (name: string): string => JSON.stringify([name]);

export function groupPayload(group: SyncGroup, order: number, now: number): Record<string, unknown> {
  return { name: group.name, order, createdAt: group.createdAt ?? now, updatedAt: now };
}

/** `order` may be an integer or a fractional-index string; integers sort
 * before strings so a mixed list still has one stable order. */
function compareOrder(a: unknown, b: unknown): number {
  const aNumber = typeof a === 'number';
  const bNumber = typeof b === 'number';
  if (aNumber && bNumber) return a - b;
  if (aNumber) return -1;
  if (bNumber) return 1;
  return String(a ?? '').localeCompare(String(b ?? ''));
}

export function activeGroupEntities(entities: SyncEntity[]): {
  groups: Map<string, SyncEntity>;
  defaultGroup?: SyncEntity;
} {
  const groups = new Map<string, SyncEntity>();
  let defaultGroup: SyncEntity | undefined;
  for (const entity of entities) {
    if (entity.deleted) continue;
    if (entity.entityType === GROUP_ENTITY_TYPE) groups.set(entity.entityId, entity);
    if (entity.entityType === SETTING_ENTITY_TYPE && entity.entityId === DEFAULT_GROUP_SETTING_ID) defaultGroup = entity;
  }
  return { groups, defaultGroup };
}

/** Groups in server order. Rows this client cannot understand are skipped so
 * one bad entity does not break the sidebar. */
export function groupsFromEntities(groups: Map<string, SyncEntity>): SyncGroup[] {
  return [...groups.values()]
    .flatMap((entity) => {
      const name = typeof entity.payload.name === 'string' ? entity.payload.name.trim() : '';
      if (!name) return [];
      const createdAt = typeof entity.payload.createdAt === 'number' ? entity.payload.createdAt : Date.parse(entity.updatedAt) || 0;
      return [{ entity, group: { id: localGroupId(entity.entityId), name, createdAt } }];
    })
    .sort((a, b) => compareOrder(a.entity.payload.order, b.entity.payload.order)
      || a.group.createdAt - b.group.createdAt
      || a.group.id.localeCompare(b.group.id))
    .map((item) => item.group);
}

export function defaultNameFromEntity(entity: SyncEntity | undefined): string | undefined {
  const name = entity && typeof entity.payload.name === 'string' ? entity.payload.name.trim() : '';
  return name || undefined;
}

/** Local group edits to push. A group the server never saw is uploaded only
 * when this cache is known to belong to the account (`mayMigrateLocal`), and a
 * group the server knew but the user deleted is pushed as a delete. The
 * renamed default group (a setting) is pushed whenever it differs from the
 * last synced value. */
export function planGroupChanges(input: {
  previous: GroupSyncState;
  local: SyncGroup[];
  defaultName: string;
  builtinDefaultName: string;
  remote: { groups: Map<string, SyncEntity>; defaultGroup?: SyncEntity };
  mayMigrateLocal: boolean;
  now: number;
  newChangeId?: () => string;
}): SyncPushChange[] {
  const newChangeId = input.newChangeId ?? (() => crypto.randomUUID());
  const changes: SyncPushChange[] = [];
  const localIds = new Set<string>();

  input.local.forEach((group, order) => {
    const id = groupEntityId(group.id);
    localIds.add(id);
    const known = input.previous.groups[id];
    const dirty = known
      ? known.hash !== groupHash(group.name, order)
      : !input.remote.groups.has(id) && input.mayMigrateLocal;
    if (!dirty) return;
    changes.push({
      changeId: newChangeId(),
      entityType: GROUP_ENTITY_TYPE,
      entityId: id,
      operation: 'upsert',
      ...(known ? { baseVersion: known.version } : {}),
      payload: groupPayload(group, order, input.now),
    });
  });
  for (const [id, known] of Object.entries(input.previous.groups)) {
    if (localIds.has(id)) continue;
    changes.push({
      changeId: newChangeId(),
      entityType: GROUP_ENTITY_TYPE,
      entityId: id,
      operation: 'delete',
      baseVersion: known.version,
      payload: {},
    });
  }

  const knownDefault = input.previous.defaultGroup;
  const defaultDirty = knownDefault
    ? knownDefault.hash !== defaultGroupHash(input.defaultName)
    : !input.remote.defaultGroup && input.mayMigrateLocal && input.defaultName !== input.builtinDefaultName;
  if (defaultDirty) {
    changes.push({
      changeId: newChangeId(),
      entityType: SETTING_ENTITY_TYPE,
      entityId: DEFAULT_GROUP_SETTING_ID,
      operation: 'upsert',
      ...(knownDefault ? { baseVersion: knownDefault.version } : {}),
      payload: { name: input.defaultName },
    });
  }
  return changes;
}

export function groupSyncStateFrom(remote: { groups: Map<string, SyncEntity>; defaultGroup?: SyncEntity }): GroupSyncState {
  const groups: Record<string, GroupSyncMeta> = {};
  groupsFromEntities(remote.groups).forEach((group, order) => {
    const id = groupEntityId(group.id);
    const entity = remote.groups.get(id);
    // Hash the remote order as the server holds it, so a later local move is
    // detected against what was actually synced rather than the array index.
    if (entity) groups[id] = { version: entity.version, hash: groupHash(group.name, order) };
  });
  const name = defaultNameFromEntity(remote.defaultGroup);
  return {
    groups,
    ...(remote.defaultGroup && name ? { defaultGroup: { version: remote.defaultGroup.version, hash: defaultGroupHash(name) } } : {}),
  };
}

/** The `groupId` a profile carries on the server: the group's entity id, or
 * null for the default group (and for any group this client does not have). */
export function profileGroupPayload(localGroup: string | undefined, groups: SyncGroup[]): string | null {
  return localGroup && groups.some((group) => group.id === localGroup) ? groupEntityId(localGroup) : null;
}

/** A profile pointing at a deleted or unknown group belongs to the default group. */
export function profileGroupFromPayload(groupId: unknown, groups: SyncGroup[]): string | undefined {
  if (typeof groupId !== 'string' || !groupId) return undefined;
  const local = localGroupId(groupId);
  return groups.some((group) => group.id === local) ? local : undefined;
}
