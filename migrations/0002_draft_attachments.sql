-- Outgoing draft attachments. Bytes live inline in SQLite — drafts are
-- single-user, capped at 25 MB per file (and 25 MB aggregate per draft).
CREATE TABLE draft_attachments (
    id            TEXT    PRIMARY KEY,                    -- uuid
    draft_id      TEXT    NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
    filename      TEXT    NOT NULL,
    content_type  TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL,
    content       BLOB    NOT NULL,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_draft_attachments_draft ON draft_attachments(draft_id);
