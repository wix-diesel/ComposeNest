#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use composenest_adapters::SystemClock;
use composenest_application::{
    Bootstrap, BootstrapRequest, BootstrapResponse, BootstrapService, ResponseEnvelope,
};
use tauri::State;

/// Returns the non-sensitive state required to initialize the desktop UI.
#[tauri::command]
fn get_bootstrap(
    request: BootstrapRequest,
    state: State<'_, Bootstrap>,
) -> ResponseEnvelope<BootstrapResponse> {
    let context = request.context();
    if let Err(error) = context.validate() {
        return ResponseEnvelope::failure(context.request_id, error);
    }
    ResponseEnvelope::success(request.request_id, state.inner().clone().into())
}

fn main() -> tauri::Result<()> {
    let bootstrap = BootstrapService::new(SystemClock).bootstrap();

    tauri::Builder::default()
        .manage(bootstrap)
        .invoke_handler(tauri::generate_handler![get_bootstrap])
        .run(tauri::generate_context!())
}
