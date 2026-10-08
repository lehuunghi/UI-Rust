ALTER TABLE rust_change_plans ADD COLUMN result_secret TEXT;
ALTER TABLE rust_change_plans ADD COLUMN finished_at TIMESTAMPTZ;
ALTER TABLE mailbox_migration_plans ADD COLUMN source_version BIGINT;
