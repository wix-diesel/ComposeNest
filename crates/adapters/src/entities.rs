//! SeaORM mappings for the existing SQLite schema.

pub(crate) mod image_resolution {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "image_resolutions")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub instance_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub spec_revision: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub image_ref: String,
        pub digest: String,
        pub image_id: String,
        pub platform: String,
        pub first_operation_id: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod instance {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "instances")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub scope_id: String,
        pub target_id: String,
        pub display_name: String,
        pub normalized_name: String,
        pub lifecycle: String,
        pub revision: i64,
        pub project_name: String,
        pub clone_source_id: Option<String>,
        pub applied_spec_revision: Option<i64>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod runtime_target {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "runtime_targets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub scope_id: String,
        pub endpoint: String,
        pub engine_id: String,
        pub platform: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod template_snapshot {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "template_snapshots")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub instance_id: String,
        pub source_revision_id: Option<String>,
        pub template_id: String,
        pub template_version: String,
        pub selected_version: String,
        pub schema_version: i64,
        pub normalization: String,
        pub semantic_hash: String,
        pub canonical_json: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod instance_spec {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "instance_specs")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub instance_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub revision: i64,
        pub selected_version: String,
        pub storage_method: String,
        pub inputs_json: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod operation {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "operations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub instance_id: String,
        pub status: String,
        pub attempt: i64,
        pub kind: String,
        pub phase: String,
        pub expected_instance_revision: i64,
        pub old_spec_revision: Option<i64>,
        pub new_spec_revision: Option<i64>,
        pub started_at: String,
        pub completed_at: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod operation_step {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "operation_steps")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub operation_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub sequence: i64,
        pub attempt: i64,
        pub command_kind: String,
        pub expected_result: String,
        pub resource_id: String,
        pub outcome: Option<String>,
        pub recorded_at: String,
        pub observed_at: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod management_scope {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "management_scopes")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub owner_id: String,
        pub root_identity: String,
        pub default_storage_method: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod port_reservation {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "port_reservations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub scope_id: String,
        pub instance_id: String,
        pub host_ip: String,
        pub protocol: String,
        pub host_port: i64,
        pub status: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod artifact {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "artifacts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub instance_id: String,
        pub spec_revision: i64,
        pub generator_version: String,
        pub manifest_hash: String,
        pub placement: String,
        pub publication_operation_id: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod artifact_file {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "artifact_files")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub artifact_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub relative_path: String,
        pub sha256: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod storage_allocation {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "storage_allocations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub instance_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub slot: String,
        pub method: String,
        pub resource_identity: String,
        pub ownership_evidence: String,
        pub ownership: String,
        pub presence: String,
        pub initialization: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod template_revision {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "template_revisions")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub template_id: String,
        pub template_version: String,
        pub schema_version: i64,
        pub normalization: String,
        pub semantic_hash: String,
        pub canonical_json: String,
        pub origin: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod template_revision_file {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "template_revision_files")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub revision_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub relative_path: String,
        pub contents: Vec<u8>,
        pub sha256: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod template_snapshot_file {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "template_snapshot_files")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub snapshot_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub relative_path: String,
        pub contents: Vec<u8>,
        pub sha256: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod port_binding {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "port_bindings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub instance_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub spec_revision: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub slot: String,
        pub host_ip: String,
        pub host_port: i64,
        pub container_port: i64,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod request_receipt {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "request_receipts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub scope_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub request_id: String,
        pub plan_id: Option<String>,
        pub confirmed_revision: i64,
        pub request_hash: String,
        pub instance_id: String,
        pub operation_id: String,
        pub created_at: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub(crate) mod runtime_observation {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "runtime_observations")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub instance_id: String,
        pub operation_id: Option<String>,
        pub container_id: Option<String>,
        pub runtime_state: String,
        pub health: Option<String>,
        pub observed_at: String,
        pub freshness: String,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
