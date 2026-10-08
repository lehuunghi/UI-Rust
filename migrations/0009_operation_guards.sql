ALTER TABLE rust_change_plans ADD COLUMN preconditions JSONB NOT NULL DEFAULT '[]';
