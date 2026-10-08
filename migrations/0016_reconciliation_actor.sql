ALTER TABLE reconciliation_runs ADD COLUMN actor_id BIGINT REFERENCES users(id);
ALTER TABLE reconciliation_items ADD COLUMN applied_at TIMESTAMPTZ;
