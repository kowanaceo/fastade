mod auth;
pub mod mcp;
mod mcp_settings;
mod model_metadata;
mod records;
mod remote_fs;
mod remote_mcp;
mod saved_sessions;
mod servers;
mod session;
mod ssh_config;
mod usage;
mod windows;

use tauri::Manager;

use auth::{
    google_auth_status, google_sign_in, google_sign_out, sync_changes, sync_push, sync_snapshot,
};
use mcp_settings::{mcp_agent_status, set_mcp_agent_enabled};
use model_metadata::{list_agent_models, refresh_session_agent_metadata};
use records::{
    create_session_record, delete_session_record, list_all_session_records, list_session_records,
    replace_session_records, update_session_record,
};
use remote_fs::list_remote_directory;
use saved_sessions::{
    delete_saved_session, list_saved_sessions, remember_saved_session_path, replace_saved_sessions,
    save_session_profile, update_saved_session,
};
use servers::{
    create_managed_server, delete_managed_server, list_managed_servers, update_managed_server,
};
use session::{
    clear_session_activity_override, close_session, create_session, get_home_directory,
    interrupt_session, list_sessions, reconnect_session, resize_terminal,
    session_activity_overrides, session_memory_usage, terminal_snapshot, update_session_agent,
    update_session_context, upload_file_to_session, write_terminal, AppState,
};
use ssh_config::list_ssh_hosts;
use usage::{get_agent_usage, get_local_accounts, get_remote_usage};
use windows::{
    fit_window_after_resize, fit_windows_after_display_change, open_session_window,
    watch_display_changes,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| match event {
            // Moving to a screen with a different scale is part of a display
            // reconfiguration too; the screen-parameters notification alone
            // can fire before the window lands on its final screen.
            tauri::WindowEvent::ScaleFactorChanged { .. } => {
                fit_windows_after_display_change(window.app_handle());
            }
            tauri::WindowEvent::Resized(_) => fit_window_after_resize(window),
            _ => {}
        })
        .setup(|app| {
            let state = AppState::load(app.handle()).map_err(std::io::Error::other)?;
            app.manage(state);
            mcp::start(app.handle().clone());
            watch_display_changes(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            create_session,
            get_home_directory,
            write_terminal,
            resize_terminal,
            terminal_snapshot,
            update_session_context,
            update_session_agent,
            list_saved_sessions,
            save_session_profile,
            remember_saved_session_path,
            update_saved_session,
            delete_saved_session,
            replace_saved_sessions,
            interrupt_session,
            reconnect_session,
            close_session,
            list_ssh_hosts,
            list_remote_directory,
            list_managed_servers,
            create_managed_server,
            update_managed_server,
            delete_managed_server,
            open_session_window,
            get_agent_usage,
            get_remote_usage,
            get_local_accounts,
            auth::put_usage_snapshot,
            auth::list_usage_snapshots,
            session_memory_usage,
            session_activity_overrides,
            clear_session_activity_override,
            mcp_agent_status,
            set_mcp_agent_enabled,
            list_agent_models,
            refresh_session_agent_metadata,
            upload_file_to_session,
            google_auth_status,
            google_sign_in,
            google_sign_out,
            sync_snapshot,
            sync_changes,
            sync_push,
            list_session_records,
            create_session_record,
            update_session_record,
            delete_session_record,
            list_all_session_records,
            replace_session_records
        ])
        .run(tauri::generate_context!())
        .expect("failed to run fastade");
}
