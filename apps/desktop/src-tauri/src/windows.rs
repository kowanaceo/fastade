use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const MAX_SESSION_WINDOWS: usize = 10;
const SESSION_WINDOW_PREFIX: &str = "session-";

#[tauri::command]
pub async fn open_session_window(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let label = format!("{SESSION_WINDOW_PREFIX}{session_id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let open_count = app
        .webview_windows()
        .keys()
        .filter(|label| label.starts_with(SESSION_WINDOW_PREFIX))
        .count();
    if open_count >= MAX_SESSION_WINDOWS {
        return Err(format!(
            "You can open at most {MAX_SESSION_WINDOWS} session windows."
        ));
    }

    let url = WebviewUrl::App(format!("index.html?session={session_id}").into());
    WebviewWindowBuilder::new(&app, label, url)
        .title("fastade session")
        .inner_size(980.0, 720.0)
        .min_inner_size(720.0, 520.0)
        // Tauri's native drag-drop handler swallows HTML5 drag events, which
        // the sidebar's session drag-to-group relies on.
        .disable_drag_drop_handler()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_window_limit_is_ten() {
        assert_eq!(MAX_SESSION_WINDOWS, 10);
    }
}
