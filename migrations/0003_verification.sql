ALTER TABLE domains ADD COLUMN verification_token VARCHAR(64);
ALTER TABLE users ADD COLUMN totp_last_step BIGINT NOT NULL DEFAULT -1;
CREATE INDEX notifications_due ON notifications(status,run_after);
CREATE INDEX sync_jobs_server ON api_sync_jobs(server_id,id);
