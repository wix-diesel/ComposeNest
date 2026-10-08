use composenest_adapters::sqlite::DatabaseWorker;
use composenest_application::{
    OperationEvent, OperationEventKind,
    create_operation::{CreateEffectError, CreateStages},
    create_state::ConfirmedCreate,
    operation_runner::ProgressSink,
};
use std::{fs, path::Path};

pub(crate) struct Stages<'a>(pub(crate) &'a Path, pub(crate) &'a str);
impl Stages<'_> {
    fn effect(&self, name: &str) {
        super::interruption::checkpoint(&format!("before:{name}"));
        // Mark external results independently of the process and database journal.
        fs::write(self.0.join(name), name).unwrap();
        super::interruption::checkpoint(&format!("after:{name}"));
    }
}
impl CreateStages for Stages<'_> {
    async fn prepare_storage(
        &self,
        _: &ConfirmedCreate,
        _: &str,
        sequence: u64,
    ) -> Result<u64, CreateEffectError> {
        self.effect("storage");
        Ok(sequence)
    }
    async fn resolve_image(&self, _: &ConfirmedCreate) -> Result<(), CreateEffectError> {
        self.effect("image");
        Ok(())
    }
    async fn publish_artifact(&self, _: &ConfirmedCreate) -> Result<(), CreateEffectError> {
        self.effect("artifact");
        Ok(())
    }
    async fn validate_config(&self) -> Result<(), CreateEffectError> {
        self.effect("config");
        Ok(())
    }
    async fn create_stopped(&self) -> Result<(), CreateEffectError> {
        self.effect("create");
        Ok(())
    }
    async fn inspect_created(&self) -> Result<String, CreateEffectError> {
        self.effect("inspect");
        Ok(self.1.into())
    }
    async fn start(&self, _: &str) -> Result<(), CreateEffectError> {
        self.effect("start");
        Ok(())
    }
    async fn wait_ready(&self, _: &str) -> Result<(), CreateEffectError> {
        self.effect("ready");
        Ok(())
    }
}

pub(crate) struct Progress<'a>(pub(crate) &'a DatabaseWorker);
impl ProgressSink for Progress<'_> {
    fn send(&self, event: OperationEvent) {
        if event.kind == OperationEventKind::Progress && event.sequence == 7 {
            super::interruption::checkpoint("before:commit");
            if std::env::var("COMPOSENEST_CRASH_BOUNDARY").as_deref() == Ok("full") {
                super::interruption::full_at_completion(self.0);
            }
        }
        if event.kind == OperationEventKind::Completed {
            super::interruption::checkpoint("after:commit");
        }
    }
}
