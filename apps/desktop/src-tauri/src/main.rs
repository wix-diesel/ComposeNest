#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use composenest_adapters::{SystemClock, sqlite::DatabaseWorker};
use composenest_application::{
    Bootstrap, BootstrapRequest, BootstrapResponse, BootstrapService, ResponseEnvelope,
    operation_recovery::RecoveryJournal, operation_runner::OperationRunner,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::{Manager, State};
mod create_commands;
use create_commands::*;
mod instance_commands;
use instance_commands::*;
mod port_commands;
use port_commands::*;
mod clone_commands;
use clone_commands::*;
mod template_commands;
use template_commands::*;
mod diagnostic_commands;
use diagnostic_commands::*;
mod settings_commands;
use settings_commands::*;
mod retained_commands;
use retained_commands::*;

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
    let database = Arc::new(DatabaseWorker::start(&management_root()?)?);
    database.recover_on_startup().map_err(|error| {
        std::io::Error::other(format!("operation recovery startup failed: {error:?}"))
    })?;
    let runner = Arc::new(OperationRunner::new());

    tauri::Builder::default()
        .manage(bootstrap)
        .manage(Arc::clone(&runner))
        .setup(move |app| {
            let backend = CreateBackend::new(
                Arc::clone(&database),
                Arc::clone(app.state::<Arc<OperationRunner>>().inner()),
                app.path().home_dir()?,
            )?;
            app.manage(Arc::new(backend));
            app.manage(TemplateBackend {
                bundled: app
                    .path()
                    .resolve("templates", tauri::path::BaseDirectory::Resource)?,
                results: tokio::sync::Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            get_settings,
            save_settings,
            diagnose_runtime,
            list_templates,
            reload_templates,
            list_instances,
            list_retained_storage,
            refresh_retained_storage,
            get_instance_detail,
            get_instance_actions,
            rename_instance,
            change_instance,
            get_instance_edit,
            edit_instance_ports,
            prepare_create,
            update_create_plan,
            view_create_plan,
            get_create_receipt,
            discard_create_plan,
            confirm_create,
            prepare_clone,
            update_clone_plan,
            view_clone_plan,
            get_clone_receipt,
            discard_clone_plan,
            confirm_clone
        ])
        .run(tauri::generate_context!())?;
    runner.shutdown(Duration::from_secs(30));
    Ok(())
}
