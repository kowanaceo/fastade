import type { AgentUsage, UsageSnapshot, UsageWindow } from './session.ts';

/** A snapshot is re-uploaded when its numbers change, or at least this often
 * so other devices can tell the host is still being watched. */
export const SNAPSHOT_REFRESH_SECONDS = 20 * 60;
/** A snapshot older than this is shown as stale rather than current. */
export const SNAPSHOT_STALE_SECONDS = 30 * 60;

/** Allowed in a server URL segment, which is where the id ends up. */
export function localHostId(deviceId: string | null): string {
  return `device-${(deviceId ?? 'local').replace(/[^A-Za-z0-9._-]/g, '-')}`;
}

export function localSnapshot(agentId: string, usage: AgentUsage | undefined, deviceId: string | null, now: number): UsageSnapshot | null {
  if (!usage || usage.status !== 'available' || !usage.windows.length) return null;
  return { agentId, hostId: localHostId(deviceId), hostLabel: 'This device', collectedAt: now, windows: usage.windows };
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
    return [{ agentId: value.agentId, hostId: value.hostId, hostLabel: typeof value.hostLabel === 'string' && value.hostLabel ? value.hostLabel : value.hostId, collectedAt: value.collectedAt, windows }];
  });
}

/** Other hosts and devices to show under each agent: the newest snapshot per
 * (agent, host), without this device's own (it has its own row already). */
export function mergeHostUsage(groups: UsageSnapshot[][], ownHostId: string): UsageSnapshot[] {
  const newest = new Map<string, UsageSnapshot>();
  for (const snapshot of groups.flat()) {
    if (snapshot.hostId === ownHostId || !snapshot.windows.length) continue;
    const key = `${snapshot.agentId}\n${snapshot.hostId}`;
    const current = newest.get(key);
    if (!current || snapshot.collectedAt > current.collectedAt) newest.set(key, snapshot);
  }
  return [...newest.values()].sort((a, b) => a.hostLabel.localeCompare(b.hostLabel));
}

export function snapshotAge(collectedAt: number, now: number): { label: string; stale: boolean } {
  const seconds = Math.max(0, now - collectedAt);
  const stale = seconds >= SNAPSHOT_STALE_SECONDS;
  if (seconds < 90) return { label: 'now', stale };
  if (seconds < 3600) return { label: `${Math.round(seconds / 60)}m ago`, stale };
  if (seconds < 86_400) return { label: `${Math.round(seconds / 3600)}h ago`, stale };
  return { label: `${Math.round(seconds / 86_400)}d ago`, stale };
}
