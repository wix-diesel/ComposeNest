#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use composenest_adapters::{SystemClock, sqlite::DatabaseWorker};
use composenest_application::{
    Bootstrap, BootstrapRequest, BootstrapResponse, BootstrapService, ResponseEnvelope,
    operation_recovery::RecoveryJournal, operation_runner::OperationRunner,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
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

fn management_root() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        composenest_adapters::windows_management_root::management_root()
    }
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from("/Library/Application Support/ComposeNest"))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from("/var/lib/composenest"))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bootstrap = BootstrapService::new(SystemClock).bootstrap();
    let database = DatabaseWorker::start(&management_root()?)?;
    database.recover_on_startup().map_err(|error| {
        std::io::Error::other(format!("operation recovery startup failed: {error:?}"))
    })?;
    let runner = Arc::new(OperationRunner::new());

    tauri::Builder::default()
        .manage(bootstrap)
        .manage(database)
        .manage(Arc::clone(&runner))
        .invoke_handler(tauri::generate_handler![get_bootstrap])
        .run(tauri::generate_context!())?;
    runner.shutdown(Duration::from_secs(30));
    Ok(())
}
