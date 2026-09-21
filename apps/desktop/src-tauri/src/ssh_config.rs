use serde::Serialize;
use std::{fs, path::PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHost {
    alias: String,
}

#[tauri::command]
pub fn list_ssh_hosts() -> Result<Vec<SshHost>, String> {
    let config_path = dirs::home_dir()
        .map(|path| path.join(".ssh").join("config"))
        .unwrap_or_else(|| PathBuf::from(".ssh/config"));
    if !config_path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(&config_path)
        .map_err(|error| format!("{}: {error}", config_path.display()))?;
    let mut aliases = Vec::new();
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((keyword, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for alias in value.split_whitespace() {
            if !has_pattern(alias) && !aliases.iter().any(|item| item == alias) {
                aliases.push(alias.to_owned());
            }
        }
    }
    Ok(aliases.into_iter().map(|alias| SshHost { alias }).collect())
}

fn has_pattern(host: &str) -> bool {
    host.contains('*') || host.contains('?') || host.starts_with('!')
}

#[cfg(test)]
mod tests {
    use super::has_pattern;

    #[test]
    fn wildcard_hosts_are_not_user_choices() {
        assert!(["*", "dev-?", "!blocked"]
            .iter()
            .all(|host| has_pattern(host)));
        assert!(!has_pattern("production"));
    }
}
