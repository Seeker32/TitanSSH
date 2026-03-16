use crate::core::session_manager::SessionManager;
use crate::models::session::SessionInfo;
use crate::storage::host_store::HostStore;
use std::sync::Mutex;
use tauri::{AppHandle, State};

#[tauri::command]
pub fn open_session(
    app: AppHandle,
    host_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<SessionInfo, String> {
    let store = HostStore::new(&app)?;
    let hosts = store.load()?;
    let host = hosts
        .into_iter()
        .find(|item| item.id == host_id)
        .ok_or_else(|| format!("Host not found: {host_id}"))?;

    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .open_session(app, host)
        .map_err(String::from)
}

#[tauri::command]
pub fn close_session(
    session_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .close_session(&session_id)
        .map_err(String::from)
}

#[tauri::command]
pub fn write_terminal(
    session_id: String,
    data: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .write_terminal(&session_id, data)
        .map_err(String::from)
}

#[tauri::command]
pub fn resize_terminal(
    session_id: String,
    cols: u32,
    rows: u32,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .resize_terminal(&session_id, cols, rows)
        .map_err(String::from)
}

#[tauri::command]
pub fn list_sessions(
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<Vec<SessionInfo>, String> {
    Ok(session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .list_sessions())
}
