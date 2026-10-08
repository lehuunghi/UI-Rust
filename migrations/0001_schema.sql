-- PostgreSQL schema derived from UI commit 6c3cb21de76c77c2af5739b463cd69fa1019207f.

-- No source credentials or customer data are included.

CREATE TABLE users (
  id BIGSERIAL PRIMARY KEY,
  parent_customer_id BIGINT  NULL,
  created_by_admin_id BIGINT  NULL,
  name VARCHAR(190) NOT NULL,
  email VARCHAR(190) NOT NULL UNIQUE,
  password_hash VARCHAR(255) NOT NULL,
  phone VARCHAR(50) NULL,
  address VARCHAR(255) NULL,
  company_name VARCHAR(190) NULL,
  two_factor_enabled SMALLINT NOT NULL DEFAULT 1,
  two_factor_method VARCHAR(40) NOT NULL DEFAULT 'email' CHECK (two_factor_method IN ('email','totp')),
  two_factor_secret VARCHAR(80) NULL,
  two_factor_confirmed_at TIMESTAMPTZ NULL,
  role VARCHAR(40) NOT NULL DEFAULT 'customer' CHECK (role IN ('admin','customer','sub_admin')),
  admin_level VARCHAR(40) NULL CHECK (admin_level IN ('super','manager','support','billing','readonly')),
  admin_permissions JSONB NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  last_login_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  suspended_by_customer SMALLINT NOT NULL DEFAULT 0
);

CREATE INDEX users_idx_21 ON users(parent_customer_id);

CREATE INDEX users_idx_22 ON users(created_by_admin_id);

CREATE INDEX users_idx_23 ON users(role);

CREATE INDEX users_idx_24 ON users(status);

CREATE TABLE packages (
  id BIGSERIAL PRIMARY KEY,
  name VARCHAR(190) NOT NULL,
  price DECIMAL(12,2) NOT NULL DEFAULT 0,
  promo_price DECIMAL(12,2) NULL,
  billing_months INT NOT NULL DEFAULT 12,
  duration_days INT NOT NULL DEFAULT 30,
  min_email_accounts INT NOT NULL DEFAULT 1,
  max_email_accounts INT NOT NULL DEFAULT 0,
  min_domains INT NOT NULL DEFAULT 1,
  max_domains INT NOT NULL DEFAULT 0,
  extra_email_price DECIMAL(12,2) NOT NULL DEFAULT 0,
  extra_domain_price DECIMAL(12,2) NOT NULL DEFAULT 0,
  max_admin_users INT NOT NULL DEFAULT 0,
  storage_per_account_mb INT NOT NULL DEFAULT 1024,
  allow_alias SMALLINT NOT NULL DEFAULT 1,
  allow_sub_admin SMALLINT NOT NULL DEFAULT 1,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL
);

CREATE TABLE subscriptions (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  package_id BIGINT  NOT NULL,
  price DECIMAL(12,2) NOT NULL,
  invoice_code VARCHAR(40) NULL UNIQUE,
  selected_email_accounts INT NOT NULL DEFAULT 0,
  selected_domains INT NOT NULL DEFAULT 0,
  base_price DECIMAL(12,2) NOT NULL DEFAULT 0,
  extra_email_count INT NOT NULL DEFAULT 0,
  extra_domain_count INT NOT NULL DEFAULT 0,
  extra_email_price DECIMAL(12,2) NOT NULL DEFAULT 0,
  extra_domain_price DECIMAL(12,2) NOT NULL DEFAULT 0,
  payment_method VARCHAR(40) NOT NULL DEFAULT 'sepay' CHECK (payment_method IN ('sepay','manual')),
  payment_status VARCHAR(40) NOT NULL DEFAULT 'unpaid' CHECK (payment_status IN ('unpaid','pending','paid','failed','refunded')),
  paid_at TIMESTAMPTZ NULL,
  activated_at TIMESTAMPTZ NULL,
  bank_transfer_note VARCHAR(190) NULL,
  sepay_transaction_id BIGINT NULL,
  sepay_reference_code VARCHAR(100) NULL,
  is_trial SMALLINT NOT NULL DEFAULT 0,
  renewal_for_subscription_id BIGINT  NULL,
  last_renewal_reminder_at TIMESTAMPTZ NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','active','expired','cancelled')),
  start_date DATE NOT NULL,
  end_date DATE NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  FOREIGN KEY(customer_id) REFERENCES users(id),
  FOREIGN KEY(package_id) REFERENCES packages(id),
  FOREIGN KEY(renewal_for_subscription_id) REFERENCES subscriptions(id) ON DELETE SET NULL
);

CREATE INDEX subscriptions_idx_30 ON subscriptions(customer_id,status,end_date);

CREATE TABLE trial_requests (
  id BIGSERIAL PRIMARY KEY,
  package_id BIGINT  NOT NULL,
  customer_id BIGINT  NULL,
  subscription_id BIGINT  NULL,
  name VARCHAR(190) NOT NULL,
  email VARCHAR(190) NOT NULL,
  phone VARCHAR(50) NOT NULL,
  address VARCHAR(255) NOT NULL,
  citizen_id VARCHAR(80) NULL,
  company_name VARCHAR(190) NULL,
  citizen_file VARCHAR(255) NULL,
  business_license_file VARCHAR(255) NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','rejected')),
  admin_note TEXT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  FOREIGN KEY(package_id) REFERENCES packages(id),
  FOREIGN KEY(customer_id) REFERENCES users(id) ON DELETE SET NULL,
  FOREIGN KEY(subscription_id) REFERENCES subscriptions(id) ON DELETE SET NULL
);

CREATE INDEX trial_requests_idx_19 ON trial_requests(status,created_at);

CREATE INDEX trial_requests_idx_20 ON trial_requests(email);

CREATE TABLE domains (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  domain_name VARCHAR(190) NOT NULL,
  stalwart_domain_id VARCHAR(190) NULL,
  verification_status VARCHAR(40) NOT NULL DEFAULT 'pending' CHECK (verification_status IN ('pending','verified','failed')),
  dns_records TEXT NULL,
  dns_check_result TEXT NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  verified_at TIMESTAMPTZ NULL,
  deleted_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  suspended_by_customer SMALLINT NOT NULL DEFAULT 0,
  live_domain VARCHAR(190) GENERATED ALWAYS AS (CASE WHEN status <> 'deleted' THEN domain_name ELSE NULL END) STORED,
  UNIQUE(live_domain),
  FOREIGN KEY(customer_id) REFERENCES users(id)
);

CREATE INDEX domains_idx_16 ON domains(customer_id,status);

CREATE TABLE email_groups (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  name VARCHAR(190) NOT NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','deleted')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  FOREIGN KEY(customer_id) REFERENCES users(id),
  live_name VARCHAR(190) GENERATED ALWAYS AS (CASE WHEN status <> 'deleted' THEN name ELSE NULL END) STORED,
  UNIQUE(customer_id,live_name)
);

CREATE INDEX email_groups_idx_9 ON email_groups(customer_id,status);

CREATE TABLE email_accounts (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  domain_id BIGINT  NOT NULL,
  group_id BIGINT  NULL,
  email VARCHAR(190) NOT NULL,
  display_name VARCHAR(190) NULL,
  local_part VARCHAR(100) NOT NULL,
  stalwart_account_id VARCHAR(190) NULL,
  storage_limit_mb INT NOT NULL,
  storage_used_bytes BIGINT  NOT NULL DEFAULT 0,
  storage_used_synced_at TIMESTAMPTZ NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  deleted_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  suspended_by_customer SMALLINT NOT NULL DEFAULT 0,
  live_email VARCHAR(190) GENERATED ALWAYS AS (CASE WHEN status <> 'deleted' THEN email ELSE NULL END) STORED,
  UNIQUE(live_email),
  FOREIGN KEY(customer_id) REFERENCES users(id),
  FOREIGN KEY(domain_id) REFERENCES domains(id),
  FOREIGN KEY(group_id) REFERENCES email_groups(id) ON DELETE SET NULL
);

CREATE INDEX email_accounts_idx_21 ON email_accounts(customer_id,status);

CREATE TABLE email_aliases (
  id BIGSERIAL PRIMARY KEY,
  email_account_id BIGINT  NOT NULL,
  customer_id BIGINT  NOT NULL,
  domain_id BIGINT  NOT NULL,
  alias_email VARCHAR(190) NOT NULL,
  local_part VARCHAR(100) NOT NULL,
  description VARCHAR(190) NULL,
  sync_status VARCHAR(40) NOT NULL DEFAULT 'pending' CHECK (sync_status IN ('pending','synced','failed')),
  last_sync_error TEXT NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  deleted_at TIMESTAMPTZ NULL,
  FOREIGN KEY(email_account_id) REFERENCES email_accounts(id),
  FOREIGN KEY(customer_id) REFERENCES users(id),
  FOREIGN KEY(domain_id) REFERENCES domains(id)
);

CREATE INDEX email_aliases_idx_16 ON email_aliases(alias_email,status);

CREATE INDEX email_aliases_idx_17 ON email_aliases(email_account_id,status);

CREATE INDEX email_aliases_idx_18 ON email_aliases(customer_id,status);

CREATE TABLE api_sync_jobs (
  id BIGSERIAL PRIMARY KEY,
  version BIGINT  NOT NULL DEFAULT 1,
  job_key VARCHAR(190) NOT NULL UNIQUE,
  job_type VARCHAR(80) NOT NULL,
  resource_id BIGINT  NOT NULL,
  payload JSONB NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','processing','done','failed')),
  attempts INT NOT NULL DEFAULT 0,
  last_error TEXT NULL,
  run_after TIMESTAMPTZ NOT NULL,
  locked_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL
);

CREATE INDEX api_sync_jobs_idx_13 ON api_sync_jobs(status,run_after);

CREATE INDEX api_sync_jobs_idx_14 ON api_sync_jobs(job_type,resource_id);

CREATE TABLE sub_admins (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  user_id BIGINT  NOT NULL,
  permissions JSONB NOT NULL,
  status VARCHAR(40) NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','deleted')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL,
  FOREIGN KEY(customer_id) REFERENCES users(id),
  FOREIGN KEY(user_id) REFERENCES users(id)
);

CREATE INDEX sub_admins_idx_9 ON sub_admins(customer_id,status);

CREATE TABLE sub_admin_group_access (
  id BIGSERIAL PRIMARY KEY,
  sub_admin_id BIGINT  NOT NULL,
  user_id BIGINT  NOT NULL,
  group_id BIGINT  NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(sub_admin_id) REFERENCES sub_admins(id) ON DELETE CASCADE,
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY(group_id) REFERENCES email_groups(id) ON DELETE CASCADE,
  UNIQUE(sub_admin_id,group_id)
);

CREATE INDEX sub_admin_group_access_idx_9 ON sub_admin_group_access(user_id);

CREATE TABLE sub_admin_domain_access (
  id BIGSERIAL PRIMARY KEY,
  sub_admin_id BIGINT  NOT NULL,
  user_id BIGINT  NOT NULL,
  domain_id BIGINT  NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(sub_admin_id) REFERENCES sub_admins(id) ON DELETE CASCADE,
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
  FOREIGN KEY(domain_id) REFERENCES domains(id) ON DELETE CASCADE,
  UNIQUE(sub_admin_id,domain_id)
);

CREATE INDEX sub_admin_domain_access_idx_9 ON sub_admin_domain_access(user_id);

CREATE TABLE settings (
  "key" VARCHAR(100) PRIMARY KEY,
  "value" TEXT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE languages (
  code VARCHAR(20) PRIMARY KEY,
  name VARCHAR(100) NOT NULL,
  native_name VARCHAR(100) NOT NULL,
  enabled SMALLINT NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL
);

CREATE TABLE email_templates (
  template_key VARCHAR(80) PRIMARY KEY,
  name VARCHAR(190) NOT NULL,
  subject VARCHAR(255) NOT NULL,
  body TEXT NOT NULL,
  enabled SMALLINT NOT NULL DEFAULT 1,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL
);

CREATE TABLE login_otps (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT  NOT NULL,
  code_hash VARCHAR(255) NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  consumed_at TIMESTAMPTZ NULL,
  ip VARCHAR(80) NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX login_otps_idx_8 ON login_otps(user_id,expires_at);

CREATE TABLE payment_transactions (
  id BIGSERIAL PRIMARY KEY,
  subscription_id BIGINT  NULL,
  provider VARCHAR(50) NOT NULL DEFAULT 'sepay',
  provider_transaction_id BIGINT NULL,
  gateway VARCHAR(100) NULL,
  account_number VARCHAR(100) NULL,
  reference_code VARCHAR(100) NULL,
  content VARCHAR(500) NULL,
  transfer_type VARCHAR(20) NULL,
  amount DECIMAL(12,2) NOT NULL DEFAULT 0,
  raw_payload JSONB NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(provider, provider_transaction_id),
  FOREIGN KEY(subscription_id) REFERENCES subscriptions(id) ON DELETE SET NULL
);

CREATE INDEX payment_transactions_idx_13 ON payment_transactions(subscription_id);

CREATE TABLE api_logs (
  id BIGSERIAL PRIMARY KEY,
  endpoint VARCHAR(255),
  request_payload TEXT,
  http_code INT,
  response_body TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX api_logs_idx_6 ON api_logs(created_at);

CREATE TABLE audit_logs (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT  NULL,
  action VARCHAR(100),
  ip VARCHAR(80),
  user_agent VARCHAR(255),
  context JSONB NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_logs_idx_7 ON audit_logs(user_id);

CREATE INDEX audit_logs_idx_8 ON audit_logs(created_at);

CREATE TABLE auth_rate_limits (
  bucket CHAR(64) PRIMARY KEY,
  attempts INT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX auth_rate_limits_idx_3 ON auth_rate_limits(expires_at);

CREATE TABLE stalwart_servers (
  id BIGSERIAL PRIMARY KEY,
  name VARCHAR(100) NOT NULL,
  base_url VARCHAR(500) NOT NULL,
  token_encrypted TEXT NULL,
  dry_run SMALLINT NOT NULL DEFAULT 1,
  active SMALLINT NOT NULL DEFAULT 1,
  is_primary SMALLINT NOT NULL DEFAULT 0,
  config_version BIGINT  NOT NULL DEFAULT 1,
  monitor_interval INT NOT NULL DEFAULT 300,
  failure_threshold INT NOT NULL DEFAULT 3,
  queue_threshold INT NOT NULL DEFAULT 1000,
  probe_ports SMALLINT NOT NULL DEFAULT 0,
  smtp_port INT NOT NULL DEFAULT 25,
  imap_port INT NOT NULL DEFAULT 993,
  worker_max_age_minutes INT NOT NULL DEFAULT 15,
  backup_max_age_hours INT NOT NULL DEFAULT 0,
  tls_warn_days INT NOT NULL DEFAULT 14,
  notify_enabled SMALLINT NOT NULL DEFAULT 0,
  notify_recipients JSONB NULL,
  edition VARCHAR(30) NULL,
  schema_hash CHAR(64) NULL,
  capabilities JSONB NULL,
  permissions JSONB NULL,
  failure_streak INT NOT NULL DEFAULT 0,
  last_check_at TIMESTAMPTZ NULL,
  next_check_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NULL
);

CREATE INDEX stalwart_servers_idx_28 ON stalwart_servers(active,next_check_at);

CREATE TABLE server_health_samples (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  mode VARCHAR(12) NOT NULL,
  status VARCHAR(20) NOT NULL,
  metrics JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX server_health_samples_idx_7 ON server_health_samples(server_id,id);

CREATE INDEX server_health_samples_idx_8 ON server_health_samples(created_at);

CREATE TABLE operational_jobs (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  kind VARCHAR(30) NOT NULL,
  payload JSONB NULL,
  actor_id BIGINT  NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'pending',
  attempts INT NOT NULL DEFAULT 0,
  run_after TIMESTAMPTZ NOT NULL,
  started_at TIMESTAMPTZ NULL,
  finished_at TIMESTAMPTZ NULL,
  result JSONB NULL,
  error VARCHAR(255) NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX operational_jobs_idx_14 ON operational_jobs(status,run_after);

CREATE INDEX operational_jobs_idx_15 ON operational_jobs(server_id,kind,status);

CREATE TABLE reconciliation_runs (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  server_version BIGINT  NOT NULL,
  mode VARCHAR(12) NOT NULL,
  status VARCHAR(20) NOT NULL,
  summary JSONB NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ NULL,
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX reconciliation_runs_idx_9 ON reconciliation_runs(server_id,id);

CREATE TABLE reconciliation_items (
  id BIGSERIAL PRIMARY KEY,
  run_id BIGINT  NOT NULL,
  kind VARCHAR(20) NOT NULL,
  local_id BIGINT  NULL,
  remote_id VARCHAR(190) NULL,
  label VARCHAR(190) NOT NULL,
  local_hash CHAR(64) NULL,
  before_data JSONB NULL,
  desired_data JSONB NULL,
  status VARCHAR(20) NOT NULL,
  detail VARCHAR(255) NULL,
  FOREIGN KEY(run_id) REFERENCES reconciliation_runs(id) ON DELETE CASCADE
);

CREATE INDEX reconciliation_items_idx_12 ON reconciliation_items(run_id,status);

CREATE TABLE operational_alerts (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  rule_key VARCHAR(80) NOT NULL,
  fingerprint CHAR(64) NOT NULL UNIQUE,
  severity VARCHAR(20) NOT NULL,
  message VARCHAR(255) NOT NULL,
  context JSONB NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'open',
  first_seen_at TIMESTAMPTZ NOT NULL,
  last_seen_at TIMESTAMPTZ NOT NULL,
  acknowledged_by BIGINT  NULL,
  acknowledged_at TIMESTAMPTZ NULL,
  resolved_at TIMESTAMPTZ NULL,
  last_notified_at TIMESTAMPTZ NULL,
  notification_error SMALLINT NOT NULL DEFAULT 0,
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX operational_alerts_idx_16 ON operational_alerts(status,last_seen_at);

CREATE TABLE operational_collections (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  server_version BIGINT  NOT NULL,
  kind VARCHAR(30) NOT NULL,
  mode VARCHAR(12) NOT NULL,
  filter_data JSONB NOT NULL,
  page_position INT  NOT NULL DEFAULT 0,
  data JSONB NOT NULL,
  total INT  NULL,
  has_more SMALLINT NOT NULL DEFAULT 0,
  status VARCHAR(20) NOT NULL,
  error VARCHAR(255) NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX operational_collections_idx_14 ON operational_collections(server_id,kind,id);

CREATE INDEX operational_collections_idx_15 ON operational_collections(created_at);

CREATE TABLE operational_change_plans (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  server_version BIGINT  NOT NULL,
  actor_id BIGINT  NOT NULL,
  kind VARCHAR(30) NOT NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'ready',
  payload JSONB NOT NULL,
  result JSONB NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL,
  started_at TIMESTAMPTZ NULL,
  finished_at TIMESTAMPTZ NULL,
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id)
);

CREATE INDEX operational_change_plans_idx_13 ON operational_change_plans(server_id,actor_id,id);

CREATE INDEX operational_change_plans_idx_14 ON operational_change_plans(status,expires_at);

CREATE TABLE operational_dns_checks (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  server_version BIGINT  NOT NULL,
  domain_id BIGINT  NOT NULL,
  local_hash CHAR(64) NOT NULL,
  status VARCHAR(20) NOT NULL,
  report JSONB NOT NULL,
  checked_at TIMESTAMPTZ NOT NULL,
  next_check_at TIMESTAMPTZ NOT NULL,
  FOREIGN KEY(server_id) REFERENCES stalwart_servers(id),
  FOREIGN KEY(domain_id) REFERENCES domains(id),
  UNIQUE(server_id,domain_id)
);

CREATE INDEX operational_dns_checks_idx_12 ON operational_dns_checks(next_check_at);

CREATE TABLE panel_sessions (
  id CHAR(64) PRIMARY KEY,
  user_id BIGINT  NOT NULL,
  ip VARCHAR(80) NOT NULL,
  device VARCHAR(255) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_seen_at TIMESTAMPTZ NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  revoked_at TIMESTAMPTZ NULL
);

CREATE INDEX panel_sessions_idx_8 ON panel_sessions(user_id,last_seen_at);

CREATE INDEX panel_sessions_idx_9 ON panel_sessions(expires_at);

CREATE TABLE panel_security_state (
  user_id BIGINT  PRIMARY KEY,
  revoked_before TIMESTAMPTZ NULL
);

CREATE TABLE mailbox_backup_profiles (
  id BIGSERIAL PRIMARY KEY,
  account_id BIGINT  NOT NULL UNIQUE,
  server_id BIGINT  NOT NULL,
  jmap_url VARCHAR(500) NOT NULL,
  username VARCHAR(254) NOT NULL,
  password_encrypted TEXT NOT NULL,
  enabled SMALLINT NOT NULL DEFAULT 0,
  restore_target SMALLINT NOT NULL DEFAULT 0,
  interval_hours INT NOT NULL DEFAULT 24,
  keep_local INT NOT NULL DEFAULT 7,
  keep_cloud INT NOT NULL DEFAULT 30,
  next_run_at TIMESTAMPTZ NULL,
  config_version INT NOT NULL DEFAULT 1,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE mailbox_backup_jobs (
  id BIGSERIAL PRIMARY KEY,
  profile_id BIGINT  NOT NULL,
  profile_version INT NOT NULL,
  actor_id BIGINT  NULL,
  kind VARCHAR(20) NOT NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'pending',
  source_artifact_id BIGINT  NULL,
  target_profile_id BIGINT  NULL,
  target_version INT NULL,
  preview_id BIGINT  NULL,
  result JSONB NULL,
  error VARCHAR(255) NULL,
  run_after TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at TIMESTAMPTZ NULL,
  finished_at TIMESTAMPTZ NULL
);

CREATE INDEX mailbox_backup_jobs_idx_16 ON mailbox_backup_jobs(status,run_after);

CREATE INDEX mailbox_backup_jobs_idx_17 ON mailbox_backup_jobs(profile_id,created_at);

CREATE TABLE mailbox_backup_artifacts (
  id BIGSERIAL PRIMARY KEY,
  profile_id BIGINT  NOT NULL,
  job_id BIGINT  NOT NULL UNIQUE,
  filename VARCHAR(190) NOT NULL UNIQUE,
  sha256 CHAR(64) NOT NULL,
  bytes BIGINT  NOT NULL,
  manifest JSONB NOT NULL,
  password_encrypted TEXT NOT NULL,
  destinations JSONB NULL,
  local_status VARCHAR(20) NOT NULL DEFAULT 'ready',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX mailbox_backup_artifacts_idx_11 ON mailbox_backup_artifacts(profile_id,created_at);

CREATE TABLE resource_placements (
  scope VARCHAR(20) NOT NULL,
  resource_id BIGINT  NOT NULL,
  server_id BIGINT  NOT NULL,
  version INT NOT NULL DEFAULT 1,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(scope,resource_id)
);

CREATE INDEX resource_placements_idx_6 ON resource_placements(server_id);

CREATE TABLE server_capacity (
  server_id BIGINT  PRIMARY KEY,
  max_accounts INT NOT NULL DEFAULT 0,
  storage_budget_gb DECIMAL(14,3) NOT NULL DEFAULT 0,
  cost_per_gb DECIMAL(14,4) NOT NULL DEFAULT 0,
  fixed_cost DECIMAL(14,2) NOT NULL DEFAULT 0,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE account_usage_history (
  account_id BIGINT  NOT NULL,
  sample_date DATE NOT NULL,
  used_bytes BIGINT  NOT NULL,
  PRIMARY KEY(account_id,sample_date)
);

CREATE INDEX account_usage_history_idx_4 ON account_usage_history(sample_date);

CREATE TABLE mail_activity_events (
  server_id BIGINT  NOT NULL,
  event_id VARCHAR(190) NOT NULL,
  occurred_at TIMESTAMPTZ NOT NULL,
  event VARCHAR(100) NOT NULL,
  sender VARCHAR(254) NOT NULL DEFAULT '',
  recipient VARCHAR(254) NOT NULL DEFAULT '',
  message_id VARCHAR(255) NOT NULL DEFAULT '',
  PRIMARY KEY(server_id,event_id)
);

CREATE INDEX mail_activity_events_idx_8 ON mail_activity_events(server_id,occurred_at);

CREATE INDEX mail_activity_events_idx_9 ON mail_activity_events(sender,occurred_at);

CREATE TABLE support_incidents (
  id BIGSERIAL PRIMARY KEY,
  customer_id BIGINT  NOT NULL,
  domain_id BIGINT  NULL,
  account_id BIGINT  NULL,
  assigned_to BIGINT  NULL,
  subject VARCHAR(190) NOT NULL,
  severity VARCHAR(20) NOT NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'open',
  root_cause TEXT NULL,
  resolution TEXT NULL,
  version INT NOT NULL DEFAULT 1,
  created_by BIGINT  NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX support_incidents_idx_14 ON support_incidents(customer_id,status);

CREATE INDEX support_incidents_idx_15 ON support_incidents(assigned_to,status);

CREATE TABLE support_incident_notes (
  id BIGSERIAL PRIMARY KEY,
  incident_id BIGINT  NOT NULL,
  actor_id BIGINT  NOT NULL,
  note TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX support_incident_notes_idx_5 ON support_incident_notes(incident_id,id);

CREATE TABLE mailbox_migration_plans (
  id BIGSERIAL PRIMARY KEY,
  domain_id BIGINT  NOT NULL,
  source_server_id BIGINT  NOT NULL,
  target_server_id BIGINT  NOT NULL,
  actor_id BIGINT  NOT NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'draft',
  inventory_hash CHAR(64) NOT NULL,
  target_version INT NOT NULL,
  mappings JSONB NOT NULL,
  checklist JSONB NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  applied_at TIMESTAMPTZ NULL
);

CREATE INDEX mailbox_migration_plans_idx_13 ON mailbox_migration_plans(domain_id,status);

CREATE TABLE native_tenant_bindings (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  customer_id BIGINT  NOT NULL,
  tenant_id VARCHAR(190) NOT NULL,
  version BIGINT  NOT NULL DEFAULT 1,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (server_id,tenant_id),
  UNIQUE (server_id,customer_id)
);

CREATE TABLE emergency_dkim_plans (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  domain_id BIGINT  NOT NULL,
  actor_id BIGINT  NOT NULL,
  server_version BIGINT  NOT NULL,
  local_hash CHAR(64) NOT NULL,
  old_key_id VARCHAR(190) NOT NULL,
  old_key JSONB NOT NULL,
  selector VARCHAR(63) NOT NULL,
  public_value TEXT NOT NULL,
  private_encrypted TEXT NULL,
  new_key_id VARCHAR(190) NULL,
  status VARCHAR(30) NOT NULL DEFAULT 'ready',
  phase VARCHAR(20) NULL,
  phase_expires_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  error VARCHAR(255) NULL
);

CREATE INDEX emergency_dkim_plans_idx_18 ON emergency_dkim_plans(server_id,id);

CREATE TABLE operational_report_history (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  kind VARCHAR(40) NOT NULL,
  fingerprint CHAR(64) NOT NULL,
  domain_name VARCHAR(253) NOT NULL DEFAULT '',
  report_id VARCHAR(190) NOT NULL,
  period_start TIMESTAMPTZ NOT NULL,
  period_end TIMESTAMPTZ NOT NULL,
  total BIGINT  NOT NULL DEFAULT 0,
  passed BIGINT  NOT NULL DEFAULT 0,
  failed BIGINT  NOT NULL DEFAULT 0,
  summary JSONB NOT NULL,
  source VARCHAR(20) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (server_id,kind,fingerprint)
);

CREATE INDEX operational_report_history_idx_15 ON operational_report_history(domain_name,period_start,period_end);

CREATE INDEX operational_report_history_idx_16 ON operational_report_history(server_id,kind,period_start);

CREATE TABLE recovery_profiles (
  id BIGSERIAL PRIMARY KEY,
  server_id BIGINT  NOT NULL,
  name VARCHAR(190) NOT NULL,
  source_key VARCHAR(100) NOT NULL,
  object_types JSONB NOT NULL,
  vault_encrypted TEXT NULL,
  enabled SMALLINT NOT NULL DEFAULT 0,
  interval_hours INT NOT NULL DEFAULT 24,
  keep_local INT NOT NULL DEFAULT 7,
  keep_cloud INT NOT NULL DEFAULT 30,
  next_run_at TIMESTAMPTZ NULL,
  restore_target SMALLINT NOT NULL DEFAULT 0,
  version BIGINT  NOT NULL DEFAULT 1,
  created_by BIGINT  NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (server_id)
);

CREATE INDEX recovery_profiles_idx_17 ON recovery_profiles(enabled,next_run_at);

CREATE TABLE recovery_jobs (
  id BIGSERIAL PRIMARY KEY,
  profile_id BIGINT  NOT NULL,
  target_profile_id BIGINT  NULL,
  artifact_id BIGINT  NULL,
  preview_id BIGINT  NULL,
  actor_id BIGINT  NOT NULL,
  kind VARCHAR(20) NOT NULL,
  status VARCHAR(25) NOT NULL DEFAULT 'pending',
  profile_version BIGINT  NOT NULL,
  target_version BIGINT  NULL,
  source_config_hash CHAR(64) NOT NULL,
  target_config_hash CHAR(64) NULL,
  result JSONB NULL,
  error VARCHAR(255) NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at TIMESTAMPTZ NULL,
  finished_at TIMESTAMPTZ NULL
);

CREATE INDEX recovery_jobs_idx_17 ON recovery_jobs(status,id);

CREATE TABLE recovery_artifacts (
  id BIGSERIAL PRIMARY KEY,
  profile_id BIGINT  NOT NULL,
  job_id BIGINT  NOT NULL,
  filename VARCHAR(190) NOT NULL,
  sha256 CHAR(64) NOT NULL,
  bytes BIGINT  NOT NULL,
  manifest JSONB NOT NULL,
  password_encrypted TEXT NOT NULL,
  destinations JSONB NOT NULL,
  local_status VARCHAR(20) NOT NULL DEFAULT 'ready',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (job_id)
);

CREATE INDEX recovery_artifacts_idx_12 ON recovery_artifacts(profile_id,id);

CREATE TABLE password_reset_tokens (
  selector CHAR(32) PRIMARY KEY,
  user_id BIGINT  NOT NULL,
  verifier_hash CHAR(64) NOT NULL,
  password_fingerprint CHAR(64) NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  used_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX password_reset_tokens_idx_7 ON password_reset_tokens(user_id);

CREATE INDEX password_reset_tokens_idx_8 ON password_reset_tokens(expires_at);

CREATE TABLE account_recovery_codes (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT  NOT NULL,
  code_hash CHAR(64) NOT NULL,
  used_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(user_id,code_hash),
  FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE notification_jobs (
  id BIGSERIAL PRIMARY KEY,
  dedup_key VARCHAR(190) NOT NULL,
  template_key VARCHAR(100) NOT NULL,
  recipient VARCHAR(254) NOT NULL,
  recipient_name VARCHAR(190) NOT NULL,
  variables_encrypted TEXT NOT NULL,
  status VARCHAR(20) NOT NULL DEFAULT 'pending',
  attempts INT NOT NULL DEFAULT 0,
  run_after TIMESTAMPTZ NOT NULL,
  locked_at TIMESTAMPTZ NULL,
  last_error VARCHAR(190) NULL,
  callback_subscription_id BIGINT  NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  sent_at TIMESTAMPTZ NULL,
  UNIQUE(dedup_key)
);

CREATE INDEX notification_jobs_idx_15 ON notification_jobs(status,run_after);

CREATE TABLE sepay_reconciliation (
  id BIGSERIAL PRIMARY KEY,
  source_key VARCHAR(100) NOT NULL,
  mode VARCHAR(10) NOT NULL DEFAULT 'live',
  source VARCHAR(20) NOT NULL,
  provider_id VARCHAR(80) NOT NULL,
  bank_identity CHAR(64) NULL,
  account_number VARCHAR(100) NOT NULL,
  reference_code VARCHAR(100) NOT NULL DEFAULT '',
  amount DECIMAL(12,2) NOT NULL,
  content VARCHAR(500) NOT NULL DEFAULT '',
  invoice_code VARCHAR(190) NOT NULL DEFAULT '',
  transaction_at TIMESTAMPTZ NULL,
  subscription_id BIGINT  NULL,
  ledger_id BIGINT  NULL,
  state VARCHAR(30) NOT NULL DEFAULT 'unmatched',
  note VARCHAR(500) NOT NULL DEFAULT '',
  reviewed_by BIGINT  NULL,
  reviewed_at TIMESTAMPTZ NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(source_key),
  UNIQUE(mode,bank_identity),
  FOREIGN KEY(subscription_id) REFERENCES subscriptions(id) ON DELETE SET NULL,
  FOREIGN KEY(ledger_id) REFERENCES payment_transactions(id) ON DELETE SET NULL
);

CREATE INDEX sepay_reconciliation_idx_21 ON sepay_reconciliation(subscription_id);

CREATE INDEX sepay_reconciliation_idx_22 ON sepay_reconciliation(state,created_at);

CREATE TABLE sepay_receipts (
  source_key VARCHAR(100) PRIMARY KEY,
  reconciliation_id BIGINT  NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  FOREIGN KEY(reconciliation_id) REFERENCES sepay_reconciliation(id) ON DELETE CASCADE
);

CREATE INDEX sepay_receipts_idx_3 ON sepay_receipts(reconciliation_id);
