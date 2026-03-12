CREATE TABLE messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    thread_id UUID REFERENCES threads(id) ON DELETE SET NULL,
    message_id_header TEXT,
    in_reply_to TEXT,
    references_header TEXT,
    from_addr TEXT NOT NULL,
    to_addrs JSONB NOT NULL DEFAULT '[]',
    cc_addrs JSONB,
    subject TEXT,
    text_body TEXT,
    html_body TEXT,
    extracted_text TEXT,
    direction TEXT NOT NULL CHECK (direction IN ('inbound', 'outbound')),
    raw_headers JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_messages_inbox_id ON messages(inbox_id);
CREATE INDEX idx_messages_thread_id ON messages(thread_id);
CREATE INDEX idx_messages_message_id_header ON messages(message_id_header);
CREATE INDEX idx_messages_created_at ON messages(created_at DESC);
