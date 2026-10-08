ALTER TABLE api_sync_jobs ADD COLUMN server_version BIGINT;
UPDATE api_sync_jobs j SET server_version=s.config_version FROM stalwart_servers s WHERE s.id=j.server_id;
