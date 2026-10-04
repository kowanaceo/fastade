import type { SessionRecord, SessionRecordDraft } from '../domain/session-record';

/** Transport-independent CRUD boundary. An aisshapi adapter must authenticate
 * every call, enforce profile ownership, and report conflicts as failures.
 * No local-only or guessed HTTP implementation is provided. */
export interface SessionRecordRepository {
  list(profileId: string): Promise<SessionRecord[]>;
  create(profileId: string, draft: SessionRecordDraft): Promise<SessionRecord>;
  update(record: SessionRecord, draft: SessionRecordDraft): Promise<SessionRecord>;
  delete(record: SessionRecord): Promise<void>;
}
