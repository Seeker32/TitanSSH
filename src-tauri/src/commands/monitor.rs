use crate::core::session_manager::SessionManager;
use crate::models::monitor::ServerStatus;
use std::sync::Mutex;
use tauri::State;

#[tauri::command]
pub fn get_server_status(
    session_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<ServerStatus, String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .get_monitor_status(&session_id)
        .map_err(String::from)
}
