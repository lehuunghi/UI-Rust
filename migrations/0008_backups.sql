CREATE TABLE rust_backup_policy (
  id BOOLEAN PRIMARY KEY DEFAULT true CHECK(id),
  enabled BOOLEAN NOT NULL DEFAULT false,
  interval_hours INT NOT NULL DEFAULT 24 CHECK(interval_hours BETWEEN 1 AND 8760),
  keep_local INT NOT NULL DEFAULT 7 CHECK(keep_local BETWEEN 1 AND 1000),
  keep_cloud INT NOT NULL DEFAULT 30 CHECK(keep_cloud BETWEEN 1 AND 10000),
  destination TEXT NOT NULL DEFAULT '',
  next_run_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO rust_backup_policy(id) VALUES(true);
CREATE TABLE rust_backups (
  id BIGSERIAL PRIMARY KEY,
  actor_id BIGINT REFERENCES users(id),
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','done','failed')),
  filename TEXT,
  checksum CHAR(64),
  bytes BIGINT,
  local_available BOOLEAN NOT NULL DEFAULT false,
  cloud_status TEXT NOT NULL DEFAULT 'none',
  cloud_path TEXT,
  cloud_attempts INT NOT NULL DEFAULT 0,
  cloud_retry_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at TIMESTAMPTZ,
  finished_at TIMESTAMPTZ
);
CREATE INDEX rust_backups_work ON rust_backups(status,created_at);
