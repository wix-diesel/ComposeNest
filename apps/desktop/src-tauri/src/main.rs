#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use composenest_adapters::{SystemClock, sqlite::DatabaseWorker};
use composenest_application::{
    Bootstrap, BootstrapRequest, BootstrapResponse, BootstrapService, ResponseEnvelope,
};
use std::path::PathBuf;
use tauri::State;

/// Returns the non-sensitive state required to initialize the desktop UI.
#[tauri::command]
fn get_bootstrap(
    request: BootstrapRequest,
    state: State<'_, Bootstrap>,
) -> ResponseEnvelope<BootstrapResponse> {
    let context = request.context();
    if let Err(error) = context.validate() {
        return ResponseEnvelope::failure(context.request_id, *error);
    }
    ResponseEnvelope::success(request.request_id, state.inner().clone().into())
}

fn management_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()))
            .join("ComposeNest")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/ComposeNest")
    }
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/var/lib/composenest")
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap = BootstrapService::new(SystemClock).bootstrap();
    let database = DatabaseWorker::start(&management_root())?;

    tauri::Builder::default()
        .manage(bootstrap)
        .manage(database)
        .invoke_handler(tauri::generate_handler![get_bootstrap])
        .run(tauri::generate_context!())?;
    Ok(())
}
