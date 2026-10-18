/** Recent selections, including clicks, group changes and newly opened sessions. */
export class SessionSelectionHistory {
  private sessionIds: string[] = [];

  record(sessionId: string | null): void {
    if (!sessionId) return;
    this.sessionIds = [sessionId, ...this.sessionIds.filter((id) => id !== sessionId)];
  }

  previous(currentId: string | null, openSessionIds: string[]): string | null {
    const open = new Set(openSessionIds);
    this.sessionIds = this.sessionIds.filter((id) => open.has(id));
    return this.sessionIds.find((id) => id !== currentId) ?? null;
  }
}
