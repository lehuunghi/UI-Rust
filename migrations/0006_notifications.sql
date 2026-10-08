-- AES-GCM ciphertext is longer than the legacy PHP SecretBox column.
ALTER TABLE users ALTER COLUMN two_factor_secret TYPE TEXT;
ALTER TABLE notifications ADD COLUMN dedupe_key TEXT UNIQUE;
CREATE TABLE rust_poll_state(kind TEXT PRIMARY KEY, next_run_at TIMESTAMPTZ NOT NULL, last_error TEXT);
INSERT INTO rust_poll_state(kind,next_run_at) VALUES('sepay',now());
