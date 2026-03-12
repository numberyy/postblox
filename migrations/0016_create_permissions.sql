CREATE TABLE permissions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id UUID NOT NULL UNIQUE REFERENCES inboxes(id) ON DELETE CASCADE,
    send_mode TEXT NOT NULL DEFAULT 'approval' CHECK (send_mode IN ('shadow', 'approval', 'auto_approve', 'autonomous')),
    rules JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
