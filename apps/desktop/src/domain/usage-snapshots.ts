import type { AgentAccount, AgentUsage, UsageSnapshot, UsageWindow } from './session.ts';

/** A snapshot is re-uploaded when its numbers change, or at least this often
 * so other devices can tell the host is still being watched. */
export const SNAPSHOT_REFRESH_SECONDS = 20 * 60;
/** A snapshot older than this is shown as stale rather than current. */
export const SNAPSHOT_STALE_SECONDS = 30 * 60;

/** Allowed in a server URL segment, which is where the id ends up. */
export function localHostId(deviceId: string | null): string {
  return `device-${(deviceId ?? 'local').replace(/[^A-Za-z0-9._-]/g, '-')}`;
}

export function localSnapshot(agentId: string, usage: AgentUsage | undefined, deviceId: string | null, now: number, accountId?: string): UsageSnapshot | null {
  if (!usage || usage.status !== 'available' || !usage.windows.length) return null;
  return { agentId, hostId: localHostId(deviceId), hostLabel: 'This device', accountId, collectedAt: now, windows: usage.windows };
}

/** What the server would learn from a snapshot, ignoring when it was taken. */
export function windowsKey(windows: UsageWindow[]): string {
  return JSON.stringify(windows.map((window) => [window.label, window.remainingPercent, window.resetsAt ?? null]));
}

export function shouldUpload(previous: { key: string; at: number } | undefined, snapshot: UsageSnapshot, now: number): boolean {
  return !previous || previous.key !== windowsKey(snapshot.windows) || now - previous.at >= SNAPSHOT_REFRESH_SECONDS;
}

/** The server answers with a bare list or `{ snapshots: [...] }`; anything
 * that is not a well-formed snapshot is skipped, never trusted. */
export function parseSnapshotList(raw: unknown): UsageSnapshot[] {
  const list = Array.isArray(raw) ? raw : (raw as { snapshots?: unknown } | null)?.snapshots;
  if (!Array.isArray(list)) return [];
  return list.flatMap((item): UsageSnapshot[] => {
    const value = item as Partial<UsageSnapshot> | null;
    if (!value || typeof value.agentId !== 'string' || typeof value.hostId !== 'string'
      || typeof value.collectedAt !== 'number' || !Array.isArray(value.windows)) return [];
    const windows = value.windows.filter((window): window is UsageWindow =>
      !!window && typeof window.label === 'string' && typeof window.remainingPercent === 'number');
    return [{ agentId: value.agentId, hostId: value.hostId, hostLabel: typeof value.hostLabel === 'string' && value.hostLabel ? value.hostLabel : value.hostId, accountId: typeof value.accountId === 'string' && value.accountId ? value.accountId : undefined, collectedAt: value.collectedAt, windows }];
  });
}

function weekReset(snapshot: UsageSnapshot): number | undefined {
  return snapshot.windows.find((window) => /week|주/i.test(window.label) && window.resetsAt)?.resetsAt;
}

/** Whether two snapshots of one agent come from the same account. Account ids
 * decide when both have one; a host that reports none (no readable login on
 * it) is matched by the weekly reset instant, which accounts do not share. */
function sameAccount(a: UsageSnapshot, b: UsageSnapshot): boolean {
  if (a.agentId !== b.agentId) return false;
  if (a.accountId && b.accountId) return a.accountId === b.accountId;
  const reset = weekReset(a);
  return reset !== undefined && reset === weekReset(b);
}

/** Other hosts and devices to show under each agent. Hosts on the same account
 * share one set of limits, so they collapse into a single entry labelled with
 * every host; windows missing from the newest snapshot are filled in from older
 * ones. This device's own account (by id, or by weekly reset when a host has no
 * id) is dropped: it has a row. */
export function mergeHostUsage(groups: UsageSnapshot[][], ownHostId: string, ownAccounts: AgentAccount[] = [], ownUsage: UsageSnapshot[] = []): UsageSnapshot[] {
  const own = new Set(ownAccounts.map((account) => `${account.agentId}\n${account.accountId}`));
  const clusters: UsageSnapshot[][] = [];
  // Snapshots with an id first, so id-less ones attach to a known account.
  const candidates = groups.flat()
    .filter((snapshot) => snapshot.hostId !== ownHostId && snapshot.windows.length
      && !(snapshot.accountId && own.has(`${snapshot.agentId}\n${snapshot.accountId}`))
      && !ownUsage.some((mine) => sameAccount(mine, snapshot)))
    .sort((a, b) => Number(!!b.accountId) - Number(!!a.accountId));
  for (const snapshot of candidates) {
    const cluster = clusters.find((list) => list.some((member) => sameAccount(member, snapshot))
      || (!snapshot.accountId && list[0].agentId === snapshot.agentId && list.some((member) => member.hostId === snapshot.hostId)));
    if (cluster) cluster.push(snapshot); else clusters.push([snapshot]);
  }
  return clusters
    .map((list) => {
      list.sort((a, b) => b.collectedAt - a.collectedAt);
      const windows = new Map<string, UsageWindow>();
      for (const snapshot of list) for (const window of snapshot.windows) if (!windows.has(window.label)) windows.set(window.label, window);
      const labels = [...new Set(list.map((snapshot) => snapshot.hostLabel))].sort();
      return { ...list[0], hostLabel: labels.join(', '), windows: [...windows.values()] };
    })
    .sort((a, b) => a.hostLabel.localeCompare(b.hostLabel));
}

export function snapshotAge(collectedAt: number, now: number): { label: string; stale: boolean } {
  const seconds = Math.max(0, now - collectedAt);
  const stale = seconds >= SNAPSHOT_STALE_SECONDS;
  if (seconds < 90) return { label: 'now', stale };
  if (seconds < 3600) return { label: `${Math.round(seconds / 60)}m ago`, stale };
  if (seconds < 86_400) return { label: `${Math.round(seconds / 3600)}h ago`, stale };
  return { label: `${Math.round(seconds / 86_400)}d ago`, stale };
}

/** Agent ids that have shared snapshots but no enabled agent row to hang them
 * on, so what the account collected is still listed rather than dropped. */
export function unlistedAgentIds(snapshots: UsageSnapshot[], listedIds: Iterable<string>): string[] {
  const listed = new Set(listedIds);
  return [...new Set(snapshots.map((snapshot) => snapshot.agentId))].filter((id) => !listed.has(id)).sort();
}
