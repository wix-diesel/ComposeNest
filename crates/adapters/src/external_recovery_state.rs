//! Durable, hash-bound recovery decisions without changing saved spec values.

use composenest_application::{operation_journal::RequestReceipt, state_store::StoreConflict};
use composenest_domain::instance::RuntimeStatus;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::{
    artifact_store::validate_id,
    sqlite::{DatabaseError, DatabaseWorker},
    state_store::map_error,
};

/// Immutable file confirmation bound to an idempotent recovery request.
#[derive(Debug, Clone)]
pub struct ExternalRecoveryRequest {
    /// Request identity; confirmed_revision is the current instance revision.
    pub receipt: RequestReceipt,
    /// Saved, applied configuration revision to regenerate.
    pub spec_revision: u64,
    /// Current generated artifact shown by the preview.
    pub source_artifact_id: String,
    /// Hash of the complete external file set explicitly confirmed by the user.
    pub confirmation_hash: String,
}

impl ExternalRecoveryRequest {
    /// Computes the non-secret request identity expected in the receipt.
    pub fn request_hash(&self) -> String {
        let mut hash = Sha256::new();
        for value in [
            &self.receipt.scope_id,
            &self.receipt.instance_id,
            &self.receipt.confirmed_revision.to_string(),
            &self.spec_revision.to_string(),
            &self.source_artifact_id,
            &self.confirmation_hash,
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        format!("{hash:x}", hash = hash.finalize())
    }
}

/// Recovery identities loaded from the accepted journal, never from external Compose.
#[derive(Debug, Clone)]
pub struct ExternalRecoveryRecord {
    /// Original externally edited artifact.
    pub source_artifact_id: String,
    /// New generated artifact, stable across attempts.
    pub replacement_artifact_id: String,
    /// Exact confirmed file set.
    pub confirmation_hash: String,
    /// Container ID observed before recovery, if present.
    pub original_container_id: Option<String>,
}

impl DatabaseWorker {
    /// Returns a recovered artifact selection, or the original deterministic artifact ID.
    pub fn selected_artifact(
        &self,
        instance: &str,
        revision: u64,
    ) -> Result<String, StoreConflict> {
        self.read(|db| {
            Ok(db.query_row("SELECT artifact_id FROM artifact_selections WHERE instance_id = ?1 AND spec_revision = ?2", params![instance, revision], |row| row.get(0)).optional()?
                .unwrap_or_else(|| format!("{instance}-r{revision}")))
        }).map_err(map_error)
    }

    /// Atomically accepts a hash-bound Recover operation and captures its original container.
    pub fn begin_external_recovery(
        &self,
        request: &ExternalRecoveryRequest,
    ) -> Result<RequestReceipt, StoreConflict> {
        let receipt = &request.receipt;
        if receipt.plan_id.is_some()
            || receipt.request_id.is_empty()
            || receipt.request_hash != request.request_hash()
            || receipt.confirmed_revision == 0
            || request.spec_revision == 0
            || receipt.operation_id.len() > 100
            || request.confirmation_hash.len() != 64
            || !request
                .confirmation_hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(StoreConflict::InvalidInput);
        }
        for id in [
            &receipt.operation_id,
            &receipt.instance_id,
            &request.source_artifact_id,
        ] {
            validate_id(id).map_err(|_| StoreConflict::InvalidInput)?;
        }
        let request = request.clone();
        self.write(move |db| {
            let tx = db.transaction()?;
            let r = &request.receipt;
            let prior: Option<(String, String, String, u64)> = tx.query_row(
                "SELECT instance_id, operation_id, request_hash, confirmed_revision FROM request_receipts WHERE scope_id = ?1 AND request_id = ?2",
                params![r.scope_id, r.request_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
            if let Some(prior) = prior {
                if prior != (r.instance_id.clone(), r.operation_id.clone(), r.request_hash.clone(), r.confirmed_revision) {
                    return Err(DatabaseError::InvalidInput);
                }
                let same: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM external_recoveries WHERE operation_id = ?1 AND source_artifact_id = ?2 AND confirmation_hash = ?3)", params![r.operation_id, request.source_artifact_id, request.confirmation_hash], |row| row.get(0))?;
                if !same { return Err(DatabaseError::InvalidInput); }
                return Ok(r.clone());
            }
            let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM instances i JOIN artifacts a ON a.instance_id = i.id AND a.spec_revision = i.applied_spec_revision WHERE i.id = ?1 AND i.scope_id = ?2 AND i.lifecycle = 'managed' AND i.revision = ?3 AND i.applied_spec_revision = ?4 AND a.id = ?5 AND a.placement = 'published' AND a.id = COALESCE((SELECT artifact_id FROM artifact_selections WHERE instance_id = i.id AND spec_revision = i.applied_spec_revision), i.id || '-r' || i.applied_spec_revision))", params![r.instance_id, r.scope_id, r.confirmed_revision, request.spec_revision, request.source_artifact_id], |row| row.get(0))?;
            if !current { return Err(DatabaseError::InvalidInput); }
            tx.execute("INSERT INTO operations (id, instance_id, kind, phase, expected_instance_revision, old_spec_revision) VALUES (?1, ?2, 'recover', 'confirm', ?3, ?4)", params![r.operation_id, r.instance_id, r.confirmed_revision, request.spec_revision])?;
            tx.execute("INSERT INTO request_receipts (scope_id, request_id, confirmed_revision, request_hash, instance_id, operation_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![r.scope_id, r.request_id, r.confirmed_revision, r.request_hash, r.instance_id, r.operation_id])?;
            tx.execute("INSERT INTO external_recoveries (operation_id, source_artifact_id, replacement_artifact_id, confirmation_hash, original_container_id) VALUES (?1, ?2, ?3, ?4, (SELECT container_id FROM runtime_observations WHERE instance_id = ?5))", params![r.operation_id, request.source_artifact_id, format!("{}-restored", r.operation_id), request.confirmation_hash, r.instance_id])?;
            tx.commit()?;
            Ok(r.clone())
        }).map_err(map_error)
    }

    /// Loads the exact accepted confirmation and stable replacement identity.
    pub fn external_recovery(
        &self,
        operation: &str,
    ) -> Result<ExternalRecoveryRecord, StoreConflict> {
        self.read(|db| {
            db.query_row("SELECT source_artifact_id, replacement_artifact_id, confirmation_hash, original_container_id FROM external_recoveries WHERE operation_id = ?1", [operation], |row| Ok(ExternalRecoveryRecord {
                source_artifact_id: row.get(0)?, replacement_artifact_id: row.get(1)?,
                confirmation_hash: row.get(2)?, original_container_id: row.get(3)?,
            })).optional()?.ok_or(DatabaseError::Missing)
        }).map_err(map_error)
    }

    /// Selects the verified replacement and commits the runtime observation in one transaction.
    /// The caller must freshly verify file hashes, storage, ownership and configuration.
    pub fn complete_external_recovery(
        &self,
        operation: &str,
        container: &str,
        status: RuntimeStatus,
    ) -> Result<(), StoreConflict> {
        if container.len() != 64 || !container.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StoreConflict::InvalidInput);
        }
        let (runtime, health) = match status {
            RuntimeStatus::Stopped => ("stopped", None),
            RuntimeStatus::Ready => ("running", Some("healthy")),
            RuntimeStatus::Preparing => ("running", Some("starting")),
            RuntimeStatus::Unhealthy => ("running", Some("unhealthy")),
            _ => return Err(StoreConflict::InvalidInput),
        };
        let operation = operation.to_owned();
        let container = container.to_owned();
        self.write(move |db| {
            let tx = db.transaction()?;
            let saved: Option<(String, u64, String, u64)> = tx.query_row("SELECT o.instance_id, o.old_spec_revision, e.replacement_artifact_id, o.attempt FROM operations o JOIN external_recoveries e ON e.operation_id = o.id JOIN instances i ON i.id = o.instance_id JOIN artifacts a ON a.id = e.replacement_artifact_id WHERE o.id = ?1 AND o.kind = 'recover' AND o.status = 'Executing' AND i.lifecycle = 'managed' AND i.revision = o.expected_instance_revision AND i.applied_spec_revision = o.old_spec_revision AND a.instance_id = i.id AND a.spec_revision = o.old_spec_revision AND a.placement = 'published' AND a.publication_operation_id = o.id", [&operation], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
            let (instance, revision, artifact, attempt) = saved.ok_or(DatabaseError::InvalidInput)?;
            let observed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_steps WHERE operation_id = ?1 AND sequence = (SELECT MAX(sequence) FROM operation_steps WHERE operation_id = ?1) AND attempt = ?2 AND command_kind = 'observe' AND resource_id = ?3 AND expected_result = 'state_observed' AND outcome = 'succeeded')", params![operation, attempt, container], |row| row.get(0))?;
            if !observed { return Err(DatabaseError::InvalidInput); }
            tx.execute("INSERT INTO artifact_selections (instance_id, spec_revision, artifact_id) VALUES (?1, ?2, ?3) ON CONFLICT(instance_id, spec_revision) DO UPDATE SET artifact_id = excluded.artifact_id", params![instance, revision, artifact])?;
            tx.execute("UPDATE instances SET revision = revision + 1 WHERE id = ?1", [&instance])?;
            tx.execute("INSERT INTO runtime_observations (instance_id, operation_id, container_id, runtime_state, health, freshness) VALUES (?1, ?2, ?3, ?4, ?5, 'fresh') ON CONFLICT(instance_id) DO UPDATE SET operation_id = excluded.operation_id, container_id = excluded.container_id, runtime_state = excluded.runtime_state, health = excluded.health, freshness = 'fresh', observed_at = CURRENT_TIMESTAMP", params![instance, operation, container, runtime, health])?;
            tx.execute("UPDATE operations SET status = 'Succeeded', phase = 'restored', completed_at = CURRENT_TIMESTAMP WHERE id = ?1", [&operation])?;
            tx.commit()?;
            Ok(())
        }).map_err(map_error)
    }
}
