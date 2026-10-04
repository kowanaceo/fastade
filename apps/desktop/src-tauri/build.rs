use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=GOOGLE_OAUTH_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=GOOGLE_OAUTH_CLIENT_SECRET");
    println!("cargo:rerun-if-changed=../../../.secrets/google-oauth-desktop.json");

    let mut client_id = env::var("GOOGLE_OAUTH_CLIENT_ID").unwrap_or_default();
    let mut client_secret = env::var("GOOGLE_OAUTH_CLIENT_SECRET").unwrap_or_default();
    if client_id.is_empty() || client_secret.is_empty() {
        let path = PathBuf::from("../../../.secrets/google-oauth-desktop.json");
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(installed) = value.get("installed") {
                    if client_id.is_empty() {
                        client_id = installed
                            .get("client_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_owned();
                    }
                    if client_secret.is_empty() {
                        client_secret = installed
                            .get("client_secret")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_owned();
                    }
                }
            }
        }
    }
    println!("cargo:rustc-env=FASTADE_GOOGLE_CLIENT_ID={client_id}");
    println!("cargo:rustc-env=FASTADE_GOOGLE_CLIENT_SECRET={client_secret}");
    tauri_build::build()
}
