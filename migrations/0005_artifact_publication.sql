ALTER TABLE artifacts ADD COLUMN publication_operation_id TEXT REFERENCES operations(id) ON DELETE RESTRICT;
CREATE INDEX artifact_publication_operation ON artifacts(publication_operation_id);
