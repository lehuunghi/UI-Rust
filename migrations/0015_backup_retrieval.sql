ALTER TABLE rust_backups ADD COLUMN retrieval_status TEXT NOT NULL DEFAULT 'none' CHECK(retrieval_status IN ('none','pending','running','done','failed'));
ALTER TABLE rust_backups ADD COLUMN retrieval_actor_id BIGINT REFERENCES users(id);
ALTER TABLE rust_backups ADD COLUMN retrieval_requested_at TIMESTAMPTZ;
ALTER TABLE rust_backups ADD COLUMN retrieval_finished_at TIMESTAMPTZ;
