use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Serialize;
use std::{fs, path::PathBuf, process::Command};

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostQr {
    name: String,
    host: String,
    port: u16,
    username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

struct ResolvedHost {
    qr: SshHostQr,
    identity_files: Vec<String>,
}

/// Let OpenSSH apply Host defaults, Include files and first-value precedence.
/// -G prints the effective settings without opening an SSH connection.
#[tauri::command]
pub async fn get_ssh_host_qr(alias: String, include_secret: bool) -> Result<SshHostQr, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if alias.starts_with('-') || !list_ssh_hosts()?.iter().any(|host| host.alias == alias) {
            return Err("Could not find the SSH config host.".to_owned());
        }
        let output = Command::new("ssh")
            .args(["-G", &alias])
            .output()
            .map_err(|error| format!("Could not resolve the SSH config: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Could not resolve the SSH config: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let mut resolved = parse_resolved_host(&alias, &String::from_utf8_lossy(&output.stdout))?;
        if include_secret {
            let home = dirs::home_dir().ok_or("Could not find the home directory.")?;
            let key_path = resolved
                .identity_files
                .iter()
                .filter_map(|path| expand_identity_path(path, &home, &resolved.qr))
                .find(|path| path.is_file())
                .ok_or(
                    "No private key file found for this SSH host. Enter credentials in the app.",
                )?;
            let bytes = fs::read(key_path)
                .map_err(|error| format!("Could not read the key file: {error}"))?;
            resolved.qr.key = Some(URL_SAFE_NO_PAD.encode(bytes));
        }
        Ok(resolved.qr)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn parse_resolved_host(alias: &str, contents: &str) -> Result<ResolvedHost, String> {
    let mut host = None;
    let mut username = None;
    let mut port = None;
    let mut identity_files = Vec::new();
    for line in contents.lines() {
        let Some((keyword, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        match keyword {
            "hostname" => host = Some(value.to_owned()),
            "user" => username = Some(value.to_owned()),
            "port" => port = value.parse::<u16>().ok().filter(|port| *port > 0),
            "identityfile" if value != "none" => identity_files.push(value.to_owned()),
            _ => {}
        }
    }
    Ok(ResolvedHost {
        qr: SshHostQr {
            name: alias.to_owned(),
            host: host
                .filter(|host| !host.is_empty())
                .ok_or("SSH config has no hostname.")?,
            username: username
                .filter(|user| !user.is_empty())
                .ok_or("SSH config has no user.")?,
            port: port.ok_or("SSH config has no valid port.")?,
            key: None,
        },
        identity_files,
    })
}

fn expand_identity_path(path: &str, home: &std::path::Path, server: &SshHostQr) -> Option<PathBuf> {
    let path = path
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(path);
    let path = if let Some(relative) = path.strip_prefix("~/") {
        home.join(relative).to_string_lossy().into_owned()
    } else {
        path.to_owned()
    };
    let mut expanded = String::new();
    let mut chars = path.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            expanded.push(ch);
            continue;
        }
        match chars.next()? {
            '%' => expanded.push('%'),
            'd' => expanded.push_str(&home.to_string_lossy()),
            'h' => expanded.push_str(&server.host),
            'n' => expanded.push_str(&server.name),
            'r' => expanded.push_str(&server.username),
            'p' => expanded.push_str(&server.port.to_string()),
            'u' => expanded.push_str(&std::env::var("USER").ok()?),
            _ => return None,
        }
    }
    Some(PathBuf::from(expanded))
}

#[cfg(test)]
mod tests {
    use super::{expand_identity_path, has_pattern, parse_resolved_host};
    use std::path::PathBuf;

    #[test]
    fn wildcard_hosts_are_not_user_choices() {
        assert!(["*", "dev-?", "!blocked"]
            .iter()
            .all(|host| has_pattern(host)));
        assert!(!has_pattern("production"));
    }

    #[test]
    fn qr_uses_resolved_connection_and_never_includes_a_key_by_default() {
        let resolved = parse_resolved_host("prod", "host prod\nuser deploy\nhostname 2001:db8::1\nport 2222\nidentityfile ~/.ssh/missing\nidentityfile ~/.ssh/my key\nidentityfile none\n").unwrap();
        assert_eq!(resolved.qr.name, "prod");
        assert_eq!(resolved.qr.host, "2001:db8::1");
        assert_eq!(resolved.qr.username, "deploy");
        assert_eq!(resolved.qr.port, 2222);
        assert_eq!(resolved.identity_files, ["~/.ssh/missing", "~/.ssh/my key"]);
        assert!(resolved.qr.key.is_none());
        assert!(serde_json::to_value(&resolved.qr)
            .unwrap()
            .get("key")
            .is_none());
    }

    #[test]
    fn qr_rejects_incomplete_connection_settings() {
        for settings in [
            "",
            "hostname example.com\nport 22",
            "hostname example.com\nuser root\nport 0",
            "hostname example.com\nuser root\nport invalid",
        ] {
            assert!(parse_resolved_host("prod", settings).is_err());
        }
    }

    #[test]
    fn identity_paths_expand_home_and_connection_tokens() {
        let resolved =
            parse_resolved_host("prod", "hostname example.com\nuser deploy\nport 2222").unwrap();
        let home = std::path::Path::new("/home/test");
        assert_eq!(
            expand_identity_path("~/.ssh/%n-%r-%h-%p-%%", home, &resolved.qr).unwrap(),
            PathBuf::from("/home/test/.ssh/prod-deploy-example.com-2222-%")
        );
        assert_eq!(
            expand_identity_path("%d/.ssh/key", home, &resolved.qr).unwrap(),
            home.join(".ssh/key")
        );
        assert!(expand_identity_path("~/.ssh/%x", home, &resolved.qr).is_none());
    }
}
