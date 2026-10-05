import type { SessionRecord, SessionRecordDraft } from '../domain/session-record';

/** Transport-independent CRUD boundary. The local adapter stores records on
 * this device; a future backend adapter must authenticate every call, enforce
 * profile ownership, and report conflicts as failures. */
export interface SessionRecordRepository {
  list(profileId: string): Promise<SessionRecord[]>;
  create(profileId: string, draft: SessionRecordDraft): Promise<SessionRecord>;
  update(record: SessionRecord, draft: SessionRecordDraft): Promise<SessionRecord>;
  delete(record: SessionRecord): Promise<void>;

  /** Every record on this device, for the sync engine. */
  listAll(): Promise<SessionRecord[]>;
  /** Replaces the whole local cache with the server's records. `backup` keeps
   * the previous file when the cache belonged to another account. */
  replaceAll(records: SessionRecord[], backup: boolean): Promise<void>;
}
