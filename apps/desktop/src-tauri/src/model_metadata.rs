use crate::{
    remote_fs::run_remote_script,
    session::{find_session, set_agent_metadata, AppState, CliKind, SessionSummary},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelOption {
    id: String,
    label: String,
    source: &'static str,
}

#[tauri::command]
pub async fn list_agent_models(
    endpoint: String,
    cli: CliKind,
    app: tauri::AppHandle,
) -> Result<Vec<AgentModelOption>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let documents = if endpoint == "local" {
            local_catalogs(cli)?
        } else {
            remote_catalogs(&endpoint, cli, &app)?
        };
        Ok(parse_catalogs(cli, &documents))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Default)]
struct DetectedMetadata {
    provider_session_id: Option<String>,
    effective_model: Option<String>,
}

/// Best-effort fallback for users who have not enabled lifecycle hooks. It
/// only considers transcripts changed since this agent launch and requires a
/// matching cwd, avoiding stale sessions from another project.
#[tauri::command]
pub async fn refresh_session_agent_metadata(
    session_id: String,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<SessionSummary, String> {
    let session = find_session(&session_id, &state)?;
    let Some(current) = session.current_agent.as_ref() else {
        return Ok(session);
    };
    let cli = current.cli;
    let launched_at = chrono::DateTime::parse_from_rfc3339(&current.detected_at)
        .map(|value| value.timestamp().max(0) as u64)
        .unwrap_or_default();
    let endpoint = session.endpoint.clone();
    let cwd = session.project_path.clone();
    let worker_app = app.clone();
    let detected = tauri::async_runtime::spawn_blocking(move || {
        if endpoint == "local" {
            detect_local_metadata(cli, &cwd, launched_at)
        } else {
            detect_remote_metadata(&endpoint, cli, &cwd, &worker_app)
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    if detected.provider_session_id.is_some() || detected.effective_model.is_some() {
        set_agent_metadata(
            &session_id,
            detected.provider_session_id,
            detected.effective_model,
            &state,
        )?;
    }
    find_session(&session_id, &state)
}

fn detect_local_metadata(
    cli: CliKind,
    cwd: &str,
    launched_at: u64,
) -> Result<DetectedMetadata, String> {
    let home = dirs::home_dir().ok_or_else(|| "could not determine home directory".to_owned())?;
    let root = match cli {
        CliKind::Codex => home.join(".codex/sessions"),
        CliKind::Claude => home.join(".claude/projects").join(cwd.replace('/', "-")),
        CliKind::Gemini => home.join(".gemini/tmp"),
    };
    let mut files = Vec::new();
    collect_transcripts(&root, transcript_depth(cli), &mut files);
    files.sort_by_key(|path| std::cmp::Reverse(modified_seconds(path).unwrap_or_default()));
    for path in files.into_iter().take(12) {
        let modified = modified_seconds(&path).unwrap_or_default();
        if launched_at > 0 && modified.saturating_add(5) < launched_at {
            continue;
        }
        if let Some(metadata) = parse_transcript(&read_transcript_edges(&path), cwd) {
            return Ok(metadata);
        }
    }
    Ok(DetectedMetadata::default())
}

fn transcript_depth(cli: CliKind) -> usize {
    match cli {
        CliKind::Codex => 4,
        CliKind::Claude => 1,
        CliKind::Gemini => 5,
    }
}

fn collect_transcripts(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 || files.len() >= 512 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_transcripts(&path, depth - 1, files);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("jsonl" | "json")
        ) {
            files.push(path);
        }
        if files.len() >= 512 {
            break;
        }
    }
}

fn modified_seconds(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn read_transcript_edges(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|value| value.len()).unwrap_or_default();
    let mut result = String::new();
    let mut first = (&mut file).take(256 * 1024);
    let _ = first.read_to_string(&mut result);
    drop(first);
    if length > 256 * 1024 {
        let _ = file.seek(SeekFrom::Start(length.saturating_sub(1024 * 1024)));
        result.push('\n');
        let _ = file.read_to_string(&mut result);
    }
    result
}

fn detect_remote_metadata(
    endpoint: &str,
    cli: CliKind,
    cwd: &str,
    app: &tauri::AppHandle,
) -> Result<DetectedMetadata, String> {
    let root = match cli {
        CliKind::Codex => "~/.codex/sessions",
        CliKind::Claude => "~/.claude/projects",
        CliKind::Gemini => "~/.gemini/tmp",
    };
    let script = format!(
        "find {root} -type f \\( -name '*.jsonl' -o -name '*.json' \\) -mmin -1440 -print 2>/dev/null | while IFS= read -r f; do stat -c '%Y %n' \"$f\" 2>/dev/null || stat -f '%m %N' \"$f\"; done | sort -nr | head -n 6 | cut -d' ' -f2- | while IFS= read -r f; do printf '\\n__FASTADE_TRANSCRIPT__\\n'; head -c 262144 \"$f\"; tail -c 1048576 \"$f\"; done"
    );
    let output = run_remote_script(endpoint, &script, app)?;
    for transcript in output.split("__FASTADE_TRANSCRIPT__").skip(1) {
        if let Some(metadata) = parse_transcript(transcript, cwd) {
            return Ok(metadata);
        }
    }
    Ok(DetectedMetadata::default())
}

fn parse_transcript(contents: &str, expected_cwd: &str) -> Option<DetectedMetadata> {
    let mut seen_cwd = None;
    let mut provider_session_id = None;
    let mut effective_model = None;
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        seen_cwd = string_at(&value, &["/payload/cwd", "/cwd"]).or(seen_cwd);
        provider_session_id = string_at(
            &value,
            &[
                "/payload/id",
                "/payload/session_id",
                "/sessionId",
                "/session_id",
            ],
        )
        .or(provider_session_id);
        effective_model =
            string_at(&value, &["/payload/model", "/message/model", "/model"]).or(effective_model);
    }
    if seen_cwd.as_deref() != Some(expected_cwd) {
        return None;
    }
    (provider_session_id.is_some() || effective_model.is_some()).then_some(DetectedMetadata {
        provider_session_id,
        effective_model,
    })
}

fn string_at(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn local_catalogs(cli: CliKind) -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or_else(|| "could not determine home directory".to_owned())?;
    let mut documents = Vec::new();
    match cli {
        CliKind::Codex => push_file(&mut documents, &home.join(".codex/models_cache.json")),
        CliKind::Claude => {
            let directory = home.join(".claude/cache/model-catalog");
            if let Ok(entries) = fs::read_dir(directory) {
                for entry in entries.flatten().take(32) {
                    let path = entry.path();
                    if path.extension().and_then(|value| value.to_str()) == Some("json") {
                        push_file(&mut documents, &path);
                    }
                }
            }
        }
        CliKind::Gemini => {
            for path in [
                home.join(".gemini/models.json"),
                home.join(".gemini/model_catalog.json"),
                home.join(".gemini/cache/models.json"),
            ] {
                push_file(&mut documents, &path);
            }
        }
    }
    Ok(documents)
}

fn push_file(documents: &mut Vec<String>, path: &Path) {
    if let Ok(contents) = fs::read_to_string(path) {
        // Model caches should remain small; keep the whole discovery request
        // bounded even if the cache directory is user-controlled.
        let used: usize = documents.iter().map(String::len).sum();
        if used + contents.len() <= 8 * 1024 * 1024 {
            documents.push(contents);
        }
    }
}

fn remote_catalogs(
    endpoint: &str,
    cli: CliKind,
    app: &tauri::AppHandle,
) -> Result<Vec<String>, String> {
    let script = match cli {
        CliKind::Codex => "test -f ~/.codex/models_cache.json && head -c 8388608 ~/.codex/models_cache.json || true",
        CliKind::Claude => "i=0; for f in ~/.claude/cache/model-catalog/*.json; do test -f \"$f\" || continue; i=$((i+1)); test \"$i\" -gt 16 && break; head -c 500000 \"$f\"; printf '\\n'; done 2>/dev/null || true",
        CliKind::Gemini => "for f in ~/.gemini/models.json ~/.gemini/model_catalog.json ~/.gemini/cache/models.json; do test -f \"$f\" && head -c 2097152 \"$f\" && printf '\\n'; done 2>/dev/null || true",
    };
    let output = run_remote_script(endpoint, script, app)?;
    if output.len() > 8 * 1024 * 1024 {
        return Err("Remote model catalog is unexpectedly large.".to_owned());
    }
    // Multiple concatenated JSON documents are accepted by the streaming parser.
    Ok(serde_json::Deserializer::from_str(&output)
        .into_iter::<Value>()
        .filter_map(Result::ok)
        .filter_map(|value| serde_json::to_string(&value).ok())
        .collect())
}

fn parse_catalogs(cli: CliKind, documents: &[String]) -> Vec<AgentModelOption> {
    let mut models = BTreeMap::<String, String>::new();
    for document in documents {
        let Ok(value) = serde_json::from_str::<Value>(document) else {
            continue;
        };
        match cli {
            CliKind::Codex => {
                if let Some(entries) = value.get("models").and_then(Value::as_array) {
                    for entry in entries {
                        if entry.get("visibility").and_then(Value::as_str) == Some("hide") {
                            continue;
                        }
                        add_model(
                            &mut models,
                            entry.get("slug").and_then(Value::as_str),
                            entry.get("display_name").and_then(Value::as_str),
                        );
                    }
                }
            }
            CliKind::Claude | CliKind::Gemini => collect_named_models(&value, &mut models),
        }
    }
    models
        .into_iter()
        .map(|(id, label)| AgentModelOption {
            id,
            label,
            source: "cache",
        })
        .collect()
}

fn collect_named_models(value: &Value, models: &mut BTreeMap<String, String>) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_named_models(item, models)),
        Value::Object(object) => {
            let id = ["modelId", "model_id", "slug", "model"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str));
            let label = ["displayName", "display_name", "name"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str));
            if id.is_some() && looks_like_model(id.unwrap_or_default()) {
                add_model(models, id, label);
            }
            object
                .values()
                .for_each(|child| collect_named_models(child, models));
        }
        _ => {}
    }
}

fn looks_like_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("claude") || lower.contains("gemini") || lower.starts_with("gpt-")
}

fn add_model(models: &mut BTreeMap<String, String>, id: Option<&str>, label: Option<&str>) {
    let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else {
        return;
    };
    models.entry(id.to_owned()).or_insert_with(|| {
        label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(id)
            .to_owned()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_visible_codex_models() {
        let documents = vec![r#"{"models":[{"slug":"gpt-a","display_name":"A","visibility":"list"},{"slug":"gpt-hidden","display_name":"Hidden","visibility":"hide"}]}"#.to_owned()];
        let models = parse_catalogs(CliKind::Codex, &documents);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-a");
        assert_eq!(models[0].label, "A");
    }

    #[test]
    fn recursively_parses_claude_catalog_without_collecting_effort_levels() {
        let documents = vec![r#"{"catalog":{"state":{"models":[{"modelId":"claude-sonnet-x","displayName":"Sonnet X"},{"id":"high","name":"High"}]}}}"#.to_owned()];
        let models = parse_catalogs(CliKind::Claude, &documents);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-sonnet-x");
    }

    #[test]
    fn transcript_metadata_requires_the_expected_working_directory() {
        let transcript = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"provider-1\",\"cwd\":\"/work/app\"}}\n",
            "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-exact\"}}\n"
        );
        let detected = parse_transcript(transcript, "/work/app").unwrap();
        assert_eq!(detected.provider_session_id.as_deref(), Some("provider-1"));
        assert_eq!(detected.effective_model.as_deref(), Some("gpt-exact"));
        assert!(parse_transcript(transcript, "/work/other").is_none());
    }
}
