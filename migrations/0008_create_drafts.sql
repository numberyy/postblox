CREATE TABLE drafts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    to_addrs JSONB NOT NULL DEFAULT '[]',
    cc_addrs JSONB,
    subject TEXT,
    text_body TEXT,
    html_body TEXT,
    in_reply_to_message_id UUID REFERENCES messages(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_drafts_inbox_id ON drafts(inbox_id);
