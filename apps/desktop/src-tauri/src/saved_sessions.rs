use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};
use tauri::Manager;
use uuid::Uuid;

/// A pinned project: a device plus the folder last used on it. Which AI agent
/// runs there is chosen inside the session, so it is intentionally not part
/// of the pin (older pins may still carry a `cli`; it is ignored).
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSession {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) last_endpoint: String,
    pub(crate) device_paths: HashMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectSummary {
    id: String,
    name: String,
    endpoint: String,
    project_path: String,
    device_paths: HashMap<String, String>,
}

pub(crate) fn mcp_projects(app: tauri::AppHandle) -> Result<Vec<ProjectSummary>, String> {
    Ok(project_summaries(read_profiles(&profiles_path(&app)?)?))
}

fn project_summaries(profiles: Vec<SavedSession>) -> Vec<ProjectSummary> {
    profiles
        .into_iter()
        .map(|profile| ProjectSummary {
            project_path: profile
                .device_paths
                .get(&profile.last_endpoint)
                .cloned()
                .unwrap_or_else(|| "~".to_owned()),
            id: profile.id,
            name: profile.name,
            endpoint: profile.last_endpoint,
            device_paths: profile.device_paths,
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionInput {
    name: String,
    endpoint: String,
    project_path: String,
}

#[tauri::command]
pub fn list_saved_sessions(app: tauri::AppHandle) -> Result<Vec<SavedSession>, String> {
    read_profiles(&profiles_path(&app)?)
}

#[tauri::command]
pub fn save_session_profile(
    input: SaveSessionInput,
    app: tauri::AppHandle,
) -> Result<SavedSession, String> {
    if input.name.trim().is_empty()
        || input.endpoint.trim().is_empty()
        || input.project_path.trim().is_empty()
    {
        return Err("name, endpoint, path are required".to_owned());
    }
    let path = profiles_path(&app)?;
    let mut profiles = read_profiles(&path)?;
    let mut device_paths = HashMap::new();
    device_paths.insert(input.endpoint.clone(), input.project_path);
    let profile = SavedSession {
        id: Uuid::new_v4().to_string(),
        name: input.name.trim().to_owned(),
        last_endpoint: input.endpoint,
        device_paths,
    };
    profiles.push(profile.clone());
    write_profiles(&path, &profiles)?;
    Ok(profile)
}

/// Records the path used the last time a pinned project was launched on a
/// given device, so the next launch on that same device needs no path input.
#[tauri::command]
pub fn remember_saved_session_path(
    id: String,
    endpoint: String,
    project_path: String,
    app: tauri::AppHandle,
) -> Result<SavedSession, String> {
    let path = profiles_path(&app)?;
    let mut profiles = read_profiles(&path)?;
    let profile = profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "saved session not found".to_owned())?;
    profile.device_paths.insert(endpoint.clone(), project_path);
    profile.last_endpoint = endpoint;
    let updated = profile.clone();
    write_profiles(&path, &profiles)?;
    Ok(updated)
}

#[tauri::command]
pub fn update_saved_session(
    id: String,
    name: String,
    endpoint: String,
    project_path: String,
    app: tauri::AppHandle,
) -> Result<SavedSession, String> {
    if name.trim().is_empty() || endpoint.trim().is_empty() || project_path.trim().is_empty() {
        return Err("name, endpoint, path are required".to_owned());
    }
    let path = profiles_path(&app)?;
    let mut profiles = read_profiles(&path)?;
    let profile = profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "saved session not found".to_owned())?;
    profile.name = name.trim().to_owned();
    profile.device_paths.insert(endpoint.clone(), project_path);
    profile.last_endpoint = endpoint;
    let updated = profile.clone();
    write_profiles(&path, &profiles)?;
    Ok(updated)
}

#[tauri::command]
pub fn delete_saved_session(id: String, app: tauri::AppHandle) -> Result<(), String> {
    let path = profiles_path(&app)?;
    let mut profiles = read_profiles(&path)?;
    let previous_len = profiles.len();
    profiles.retain(|profile| profile.id != id);
    if profiles.len() == previous_len {
        return Err("saved session not found".to_owned());
    }
    write_profiles(&path, &profiles)
}

fn profiles_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("saved-sessions.json"))
        .map_err(|error| error.to_string())
}

fn read_profiles(path: &PathBuf) -> Result<Vec<SavedSession>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let raw: Vec<serde_json::Value> = serde_json::from_str(&contents)
        .map_err(|error| format!("saved session data is invalid: {error}"))?;
    Ok(raw.into_iter().filter_map(parse_profile).collect())
}

/// Parses a saved profile from loosely-typed JSON so pins written by the
/// previous single-device schema (flat `endpoint`/`projectPath`) still load,
/// folded into `deviceById` under that same device.
fn parse_profile(entry: serde_json::Value) -> Option<SavedSession> {
    let id = entry.get("id")?.as_str()?.to_owned();
    let name = entry.get("name")?.as_str()?.to_owned();

    let mut device_paths: HashMap<String, String> = entry
        .get("devicePaths")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();

    let legacy_endpoint = entry
        .get("endpoint")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let legacy_path = entry
        .get("projectPath")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    if let (Some(endpoint), Some(project_path)) = (&legacy_endpoint, &legacy_path) {
        device_paths
            .entry(endpoint.clone())
            .or_insert_with(|| project_path.clone());
    }

    let last_endpoint = entry
        .get("lastEndpoint")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or(legacy_endpoint)
        .or_else(|| device_paths.keys().next().cloned())?;

    Some(SavedSession {
        id,
        name,
        last_endpoint,
        device_paths,
    })
}

fn write_profiles(path: &PathBuf, profiles: &[SavedSession]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let contents = serde_json::to_string_pretty(profiles).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_profile;

    #[test]
    fn migrates_legacy_single_device_shape() {
        let legacy = serde_json::json!({
            "id": "abc",
            "name": "fastade",
            "cli": "codex",
            "model": null,
            "endpoint": "local",
            "projectPath": "/Users/example/fastade",
        });
        let profile = parse_profile(legacy).expect("legacy profile should parse");
        assert_eq!(profile.last_endpoint, "local");
        assert_eq!(
            profile.device_paths.get("local").map(String::as_str),
            Some("/Users/example/fastade")
        );
    }
}
