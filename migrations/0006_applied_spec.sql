ALTER TABLE instances ADD COLUMN applied_spec_revision INTEGER
    CHECK (applied_spec_revision IS NULL OR applied_spec_revision > 0);
