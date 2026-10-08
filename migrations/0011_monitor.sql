CREATE TABLE rust_worker_state(id INT PRIMARY KEY CHECK(id=1), heartbeat_at TIMESTAMPTZ NOT NULL);
ALTER TABLE stalwart_servers ADD COLUMN usage_monitor SMALLINT NOT NULL DEFAULT 0 CHECK(usage_monitor IN (0,1));
