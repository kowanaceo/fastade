use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;
use uuid::Uuid;

/// Notes and tasks attached to a saved session profile. They are stored only
/// on this device; nothing here talks to the backend.
static STORE_LOCK: Mutex<()> = Mutex::new(());

const STATUSES: [&str; 3] = ["todo", "in progress", "done"];

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    id: String,
    profile_id: String,
    kind: String,
    title: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecordDraft {
    kind: String,
    title: String,
    content: String,
    status: Option<String>,
}

struct Validated {
    kind: String,
    title: String,
    content: String,
    status: Option<String>,
}

fn validate(draft: SessionRecordDraft) -> Result<Validated, String> {
    let title = draft.title.trim().to_owned();
    if title.is_empty() {
        return Err("Title is required.".to_owned());
    }
    let status = match (draft.kind.as_str(), draft.status) {
        ("note", None) => None,
        ("note", Some(_)) => return Err("Notes do not have a status.".to_owned()),
        ("task", Some(status)) if STATUSES.contains(&status.as_str()) => Some(status),
        ("task", _) => return Err("Choose a valid task status.".to_owned()),
        _ => return Err("Choose a note or task.".to_owned()),
    };
    Ok(Validated {
        kind: draft.kind,
        title,
        content: draft.content,
        status,
    })
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn list_session_records(
    profile_id: String,
    app: tauri::AppHandle,
) -> Result<Vec<SessionRecord>, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let mut records: Vec<_> = read_records(&records_path(&app)?)?
        .into_iter()
        .filter(|record| record.profile_id == profile_id)
        .collect();
    records.sort_by_key(|record| record.created_at);
    Ok(records)
}

#[tauri::command]
pub fn create_session_record(
    profile_id: String,
    draft: SessionRecordDraft,
    app: tauri::AppHandle,
) -> Result<SessionRecord, String> {
    if profile_id.trim().is_empty() {
        return Err("A saved session is required.".to_owned());
    }
    let valid = validate(draft)?;
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let path = records_path(&app)?;
    let mut records = read_records(&path)?;
    let now = now_millis();
    let record = SessionRecord {
        id: Uuid::new_v4().to_string(),
        profile_id,
        kind: valid.kind,
        title: valid.title,
        content: valid.content,
        status: valid.status,
        created_at: now,
        updated_at: now,
    };
    records.push(record.clone());
    write_records(&path, &records)?;
    Ok(record)
}

#[tauri::command]
pub fn update_session_record(
    id: String,
    draft: SessionRecordDraft,
    app: tauri::AppHandle,
) -> Result<SessionRecord, String> {
    let valid = validate(draft)?;
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let path = records_path(&app)?;
    let mut records = read_records(&path)?;
    let record = records
        .iter_mut()
        .find(|record| record.id == id)
        .ok_or_else(|| "The note or task no longer exists.".to_owned())?;
    if record.kind != valid.kind {
        return Err("A note cannot be changed into a task.".to_owned());
    }
    record.title = valid.title;
    record.content = valid.content;
    record.status = valid.status;
    record.updated_at = now_millis().max(record.updated_at + 1);
    let updated = record.clone();
    write_records(&path, &records)?;
    Ok(updated)
}

#[tauri::command]
pub fn delete_session_record(id: String, app: tauri::AppHandle) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let path = records_path(&app)?;
    let mut records = read_records(&path)?;
    let previous_len = records.len();
    records.retain(|record| record.id != id);
    if records.len() == previous_len {
        return Err("The note or task no longer exists.".to_owned());
    }
    write_records(&path, &records)
}

/// Every record on this device, for the sync engine. Unlike the per-profile
/// list this is unfiltered so unsent local edits can be compared with the
/// server snapshot.
#[tauri::command]
pub fn list_all_session_records(app: tauri::AppHandle) -> Result<Vec<SessionRecord>, String> {
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    read_records(&records_path(&app)?)
}

/// Replaces the local cache with the server's records after a sync. When the
/// cache belonged to a different account the old file is kept as a backup.
#[tauri::command]
pub fn replace_session_records(
    records: Vec<SessionRecord>,
    backup: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    validate_all(&records)?;
    let _guard = STORE_LOCK.lock().map_err(|error| error.to_string())?;
    let path = records_path(&app)?;
    if backup && path.exists() {
        let copy = path.with_file_name(format!("session-records.{}.bak.json", now_millis()));
        fs::copy(&path, copy).map_err(|error| error.to_string())?;
    }
    write_records(&path, &records)
}

fn validate_all(records: &[SessionRecord]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for record in records {
        if record.id.trim().is_empty() || record.profile_id.trim().is_empty() {
            return Err("synced notes are invalid".to_owned());
        }
        if !ids.insert(record.id.as_str()) {
            return Err("synced notes contain duplicate ids".to_owned());
        }
        validate(SessionRecordDraft {
            kind: record.kind.clone(),
            title: record.title.clone(),
            content: String::new(),
            status: record.status.clone(),
        })?;
    }
    Ok(())
}

fn records_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("session-records.json"))
        .map_err(|error| error.to_string())
}

fn read_records(path: &PathBuf) -> Result<Vec<SessionRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| format!("note data is invalid: {error}"))
}

fn write_records(path: &PathBuf, records: &[SessionRecord]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = serde_json::to_string_pretty(records).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        read_records, validate, validate_all, write_records, SessionRecord, SessionRecordDraft,
    };

    fn draft(kind: &str, title: &str, status: Option<&str>) -> SessionRecordDraft {
        SessionRecordDraft {
            kind: kind.to_owned(),
            title: title.to_owned(),
            content: "  code\n\n".to_owned(),
            status: status.map(str::to_owned),
        }
    }

    #[test]
    fn notes_keep_content_verbatim_and_reject_a_status() {
        let note = validate(draft("note", "  Design  ", None)).unwrap();
        assert_eq!(note.title, "Design");
        assert_eq!(note.content, "  code\n\n");
        assert!(validate(draft("note", "Design", Some("done"))).is_err());
    }

    #[test]
    fn tasks_need_one_of_the_three_statuses() {
        for status in ["todo", "in progress", "done"] {
            assert!(validate(draft("task", "Ship", Some(status))).is_ok());
        }
        for status in [None, Some(""), Some("pending"), Some("in_progress")] {
            assert!(validate(draft("task", "Ship", status)).is_err());
        }
    }

    #[test]
    fn blank_titles_and_unknown_kinds_are_rejected() {
        assert!(validate(draft("note", " \n ", None)).is_err());
        assert!(validate(draft("other", "Title", None)).is_err());
    }

    #[test]
    fn records_round_trip_through_the_file() {
        let directory =
            std::env::temp_dir().join(format!("fastade-records-{}", uuid::Uuid::new_v4()));
        let path = directory.join("session-records.json");
        assert!(read_records(&path).unwrap().is_empty());
        let record = SessionRecord {
            id: "r1".to_owned(),
            profile_id: "p1".to_owned(),
            kind: "task".to_owned(),
            title: "Ship".to_owned(),
            content: "a\n b".to_owned(),
            status: Some("todo".to_owned()),
            created_at: 1,
            updated_at: 2,
        };
        write_records(&path, &[record]).unwrap();
        let loaded = read_records(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "a\n b");
        assert_eq!(loaded[0].status.as_deref(), Some("todo"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn synced_records_must_be_valid_and_unique() {
        let record = |id: &str, kind: &str, status: Option<&str>| SessionRecord {
            id: id.to_owned(),
            profile_id: "p".to_owned(),
            kind: kind.to_owned(),
            title: "t".to_owned(),
            content: String::new(),
            status: status.map(str::to_owned),
            created_at: 1,
            updated_at: 1,
        };
        assert!(
            validate_all(&[record("a", "note", None), record("b", "task", Some("done"))]).is_ok()
        );
        assert!(validate_all(&[record("a", "note", None), record("a", "note", None)]).is_err());
        assert!(validate_all(&[record("a", "task", None)]).is_err());
        assert!(validate_all(&[record("", "note", None)]).is_err());
    }
}
