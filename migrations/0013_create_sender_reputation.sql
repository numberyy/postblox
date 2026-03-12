CREATE TABLE sender_reputation (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    sender_email TEXT NOT NULL,
    total_messages INTEGER NOT NULL DEFAULT 0,
    slop_count INTEGER NOT NULL DEFAULT 0,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(org_id, sender_email)
);
CREATE INDEX idx_sender_reputation_org ON sender_reputation(org_id);
