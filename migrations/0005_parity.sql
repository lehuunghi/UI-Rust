ALTER TABLE panel_sessions ADD COLUMN impersonator_session CHAR(64);
CREATE INDEX panel_sessions_impersonator ON panel_sessions(impersonator_session);
CREATE TABLE trial_documents (
  trial_id BIGINT NOT NULL REFERENCES trial_requests(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('citizen','business_license')),
  mime TEXT NOT NULL CHECK(mime IN ('image/jpeg','image/png','application/pdf')),
  encrypted TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(trial_id,kind)
);
CREATE TABLE trial_identity (
  trial_id BIGINT PRIMARY KEY REFERENCES trial_requests(id) ON DELETE CASCADE,
  encrypted TEXT NOT NULL
);
INSERT INTO settings(key,value) VALUES('trial_require_identity','1'),('trial_retention_days','90') ON CONFLICT DO NOTHING;
