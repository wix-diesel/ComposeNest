//! Settings transport uses the backend scope and resolved root without Docker access.

use crate::create_commands::{CreateBackend, envelope};
use composenest_application::{
    RequestContext, ResponseEnvelope,
    create_plan::{get_default_storage, set_default_storage},
    settings::{SaveSettingsRequest, SettingsView},
    state_store::StorageMethod,
};
use std::sync::Arc;
use tauri::State;

fn settings(
    context: RequestContext,
    backend: &CreateBackend,
    method: Option<StorageMethod>,
) -> ResponseEnvelope<SettingsView> {
    if let Err(error) = context.validate() {
        return ResponseEnvelope::failure(context.request_id, *error);
    }
    let result = match method {
        Some(method) => set_default_storage(&*backend.database, &backend.scope, method),
        None => get_default_storage(&*backend.database, &backend.scope),
    }
    .map(|storage_method| SettingsView {
        storage_method,
        management_root: backend
            .database
            .management_root()
            .to_string_lossy()
            .into_owned(),
    });
    envelope(context, result)
}

/// Reads the persisted default and fixed root without probing Docker.
#[tauri::command]
pub async fn get_settings(
    request: RequestContext,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<SettingsView>, ()> {
    Ok(settings(request, &state, None))
}

/// Persists and reads back the scoped default before reporting success.
#[tauri::command]
pub async fn save_settings(
    request: SaveSettingsRequest,
    state: State<'_, Arc<CreateBackend>>,
) -> Result<ResponseEnvelope<SettingsView>, ()> {
    Ok(settings(
        request.context,
        &state,
        Some(request.storage_method),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use composenest_adapters::sqlite::DatabaseWorker;
    use composenest_application::operation_runner::OperationRunner;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn backend(root: &std::path::Path) -> CreateBackend {
        for path in [root.to_path_buf(), root.join("state"), root.join("locks")] {
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        CreateBackend::new(
            Arc::new(DatabaseWorker::start(root).unwrap()),
            Arc::new(OperationRunner::new()),
            root.join("home"),
        )
        .unwrap()
    }

    fn context(version: u16) -> RequestContext {
        RequestContext {
            api_version: version,
            request_id: "settings-test".into(),
        }
    }

    #[test]
    fn settings_read_back_persist_and_reject_invalid_context_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let state = backend(root.path());
        let initial = settings(context(1), &state, None).result.unwrap();
        assert_eq!(initial.storage_method, StorageMethod::Bind);
        assert_eq!(initial.management_root, root.path().to_string_lossy());
        assert!(
            settings(context(2), &state, Some(StorageMethod::Volume))
                .error
                .is_some()
        );
        assert_eq!(settings(context(1), &state, None).result.unwrap(), initial);
        let saved = settings(context(1), &state, Some(StorageMethod::Volume));
        assert_eq!(saved.request_id, "settings-test");
        assert_eq!(saved.result.unwrap().storage_method, StorageMethod::Volume);
        drop(state);
        let reopened = backend(root.path());
        assert_eq!(
            settings(context(1), &reopened, None)
                .result
                .unwrap()
                .storage_method,
            StorageMethod::Volume
        );
    }

    #[test]
    fn failed_store_read_or_write_never_returns_success() {
        let root = tempfile::tempdir().unwrap();
        let state = backend(root.path());
        state.database.write(|db| {
            db.execute_batch("CREATE TRIGGER reject_settings BEFORE UPDATE ON management_scopes BEGIN SELECT RAISE(ABORT, 'private database failure'); END")?;
            Ok(())
        }).unwrap();
        let failed = settings(context(1), &state, Some(StorageMethod::Volume));
        assert!(failed.result.is_none());
        assert_eq!(failed.error.unwrap().code, "STORE_UNAVAILABLE");
        assert_eq!(
            settings(context(1), &state, None)
                .result
                .unwrap()
                .storage_method,
            StorageMethod::Bind
        );
        state
            .database
            .write(|db| {
                db.execute_batch("DROP TABLE management_scopes")?;
                Ok(())
            })
            .unwrap();
        assert!(settings(context(1), &state, None).result.is_none());
    }
}
