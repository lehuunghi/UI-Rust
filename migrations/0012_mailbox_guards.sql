ALTER TABLE mailbox_backup_profiles ADD COLUMN server_version BIGINT;
ALTER TABLE mailbox_backup_profiles ADD COLUMN account_remote_id VARCHAR(190);
CREATE UNIQUE INDEX mailbox_restore_once ON mailbox_backup_jobs(preview_id) WHERE kind='restore';
CREATE UNIQUE INDEX recovery_restore_once ON recovery_jobs(preview_id) WHERE kind='restore';
