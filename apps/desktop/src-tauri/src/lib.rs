mod auth;
pub mod mcp;
mod mcp_settings;
mod model_metadata;
mod records;
mod remote_fs;
mod remote_mcp;
mod remote_shell;
mod saved_sessions;
mod servers;
mod session;
mod ssh_config;
mod stt;
mod phone;
mod tunnel;
mod usage;
mod windows;

use tauri::Manager;

use auth::{
    google_auth_status, google_sign_in, google_sign_out, sync_changes, sync_push, sync_snapshot,
};
use mcp_settings::{mcp_agent_status, set_mcp_agent_enabled};
use model_metadata::{list_agent_models, refresh_session_agent_metadata};
use records::{
    claim_todo_tasks, create_session_record, delete_session_record, list_all_session_records,
    list_session_records, replace_session_records, restore_claimed_tasks, update_session_record,
};
use remote_fs::list_remote_directory;
use saved_sessions::{
    delete_saved_session, list_saved_sessions, remember_saved_session_path, replace_saved_sessions,
    save_session_profile, update_saved_session,
};
use servers::{
    create_managed_server, delete_managed_server, get_server_qr_credentials, list_managed_servers,
    update_managed_server,
};
use session::{
    clear_session_activity_override, close_session, close_temporary_sessions, create_session,
    get_home_directory, interrupt_session, list_sessions, reconnect_session, resize_terminal,
    session_activity_overrides, session_memory_usage, terminal_snapshot, update_session_agent,
    update_session_context, upload_file_to_session, write_terminal, AppState,
};
use ssh_config::{get_ssh_host_qr, list_ssh_hosts};
use usage::{get_agent_usage, get_local_accounts, get_remote_usage};
use windows::{
    fit_window_after_resize, fit_windows_after_display_change, open_session_window,
    watch_display_changes,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(stt::VoiceRequests::default())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| match event {
            // Moving to a screen with a different scale is part of a display
            // reconfiguration too; the screen-parameters notification alone
            // can fire before the window lands on its final screen.
            tauri::WindowEvent::ScaleFactorChanged { .. } => {
                fit_windows_after_display_change(window.app_handle());
            }
            tauri::WindowEvent::Resized(_) => fit_window_after_resize(window),
            tauri::WindowEvent::Destroyed => close_temporary_sessions(window),
            _ => {}
        })
        .setup(|app| {
            let state = AppState::load(app.handle()).map_err(std::io::Error::other)?;
            app.manage(state);
            app.manage(tunnel::TunnelManager::start(app.handle().clone())?);
            mcp::start(app.handle().clone());
            watch_display_changes(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            mcp::set_group_layout,
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
            session::launch_session_agent,
            list_ssh_hosts,
            get_ssh_host_qr,
            list_remote_directory,
            list_managed_servers,
            create_managed_server,
            update_managed_server,
            delete_managed_server,
            get_server_qr_credentials,
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
            stt::transcribe_voice,
            stt::cancel_voice_transcription,
            tunnel::tunnel_status,
            tunnel::save_tunnel_settings,
            tunnel::set_tunnel_enabled,
            phone::create_phone_qr,
            phone::list_phone_keys,
            phone::revoke_phone_key,
            sync_snapshot,
            sync_changes,
            sync_push,
            list_session_records,
            create_session_record,
            update_session_record,
            claim_todo_tasks,
            restore_claimed_tasks,
            delete_session_record,
            list_all_session_records,
            replace_session_records
        ])
        .build(tauri::generate_context!())
        .expect("failed to build fastade")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                let manager = app.state::<tunnel::TunnelManager>().inner().clone();
                if manager.exit_ready() {
                    return;
                }
                api.prevent_exit();
                if manager.begin_shutdown() {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        manager.stop().await;
                        manager.finish_shutdown();
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
        });
}
