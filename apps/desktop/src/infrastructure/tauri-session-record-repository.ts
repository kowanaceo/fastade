import { invoke } from '@tauri-apps/api/core';
import type { SessionRecordRepository } from '../application/session-record-repository';
import type { SessionRecord, SessionRecordDraft } from '../domain/session-record';

/** Stores notes and tasks in this device's app data directory only. */
export class TauriSessionRecordRepository implements SessionRecordRepository {
  list(profileId: string): Promise<SessionRecord[]> {
    return invoke<SessionRecord[]>('list_session_records', { profileId });
  }

  create(profileId: string, draft: SessionRecordDraft): Promise<SessionRecord> {
    return invoke<SessionRecord>('create_session_record', { profileId, draft });
  }

  update(record: SessionRecord, draft: SessionRecordDraft): Promise<SessionRecord> {
    return invoke<SessionRecord>('update_session_record', { id: record.id, draft });
  }

  delete(record: SessionRecord): Promise<void> {
    return invoke<void>('delete_session_record', { id: record.id });
  }

  listAll(): Promise<SessionRecord[]> {
    return invoke<SessionRecord[]>('list_all_session_records');
  }

  replaceAll(records: SessionRecord[], backup: boolean): Promise<void> {
    return invoke<void>('replace_session_records', { records, backup });
  }
}
