/** Records belong to a saved session profile, whose ID survives reconnects
 * and is shared across devices, rather than to a machine's runtime PTY ID. */
export type TaskStatus = 'todo' | 'in progress' | 'done';
export type SessionRecordDraft =
  | { kind: 'note'; title: string; content: string }
  | { kind: 'task'; title: string; content: string; status: TaskStatus };

export type SessionRecord = SessionRecordDraft & {
  id: string;
  profileId: string;
  /** Milliseconds since the Unix epoch. */
  createdAt: number;
  updatedAt: number;
};

export const TASK_STATUSES: readonly TaskStatus[] = ['todo', 'in progress', 'done'];

/** Shared by direct editing and a future backend/MCP adapter. Content is
 * preserved verbatim, including indentation and trailing newlines. */
export function validateRecordDraft(value: unknown): SessionRecordDraft {
  if (!value || typeof value !== 'object') throw new Error('Invalid record.');
  const input = value as Record<string, unknown>;
  if (input.kind !== 'note' && input.kind !== 'task') throw new Error('Choose a note or task.');
  if (typeof input.title !== 'string' || !input.title.trim()) throw new Error('Title is required.');
  if (typeof input.content !== 'string') throw new Error('Content must be text.');
  const fields = { title: input.title.trim(), content: input.content };
  if (input.kind === 'note') {
    if (input.status !== undefined) throw new Error('Notes do not have a status.');
    return { kind: 'note', ...fields };
  }
  if (!TASK_STATUSES.includes(input.status as TaskStatus)) throw new Error('Choose a valid task status.');
  return { kind: 'task', ...fields, status: input.status as TaskStatus };
}
