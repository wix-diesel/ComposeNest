ALTER TABLE instances ADD COLUMN container_name TEXT;
CREATE UNIQUE INDEX unique_active_container_name ON instances(target_id, container_name)
    WHERE lifecycle != 'retired' AND container_name IS NOT NULL;
