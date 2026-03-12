CREATE INDEX IF NOT EXISTS idx_audit_log_org_created ON audit_log (org_id, created_at DESC);
