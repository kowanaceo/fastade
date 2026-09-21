pub mod mcp;
mod mcp_settings;
mod remote_fs;
mod saved_sessions;
mod servers;
mod session;
mod ssh_config;
mod usage;
mod windows;

use tauri::Manager;

use mcp_settings::{mcp_agent_status, set_mcp_agent_enabled};
use remote_fs::list_remote_directory;
use saved_sessions::{
    delete_saved_session, list_saved_sessions, remember_saved_session_path, save_session_profile,
    update_saved_session,
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
use usage::get_agent_usage;
use windows::open_session_window;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = AppState::load(app.handle()).map_err(std::io::Error::other)?;
            app.manage(state);
            mcp::start(app.handle().clone());
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
            session_memory_usage,
            session_activity_overrides,
            clear_session_activity_override,
            mcp_agent_status,
            set_mcp_agent_enabled,
            upload_file_to_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run fastade");
}
