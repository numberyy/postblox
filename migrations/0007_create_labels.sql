CREATE TABLE labels (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    color TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_labels_inbox_name ON labels(inbox_id, name);
CREATE INDEX idx_labels_inbox_id ON labels(inbox_id);

CREATE TABLE message_labels (
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    label_id UUID NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (message_id, label_id)
);

CREATE INDEX idx_message_labels_label ON message_labels(label_id);
