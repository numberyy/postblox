-- postblox — initial schema.
--
-- One SQLite database per user. Mirrors IMAP folders 1:1 — same message
-- in two folders = two rows. UUID text primary keys for stable external
-- identifiers; rowid still backs FTS5.
--
-- Secrets (passwords / OAuth refresh tokens) NEVER live here. The
-- accounts.secret_ref column points at an OS keyring item.

CREATE TABLE accounts (
    id              TEXT    PRIMARY KEY,                 -- uuid
    email           TEXT    NOT NULL UNIQUE,
    display_name    TEXT,
    auth_kind       TEXT    NOT NULL,                    -- 'password' | 'oauth2_google'
    imap_host       TEXT    NOT NULL,
    imap_port       INTEGER NOT NULL,
    imap_use_tls    INTEGER NOT NULL DEFAULT 1,          -- bool
    smtp_host       TEXT    NOT NULL,
    smtp_port       INTEGER NOT NULL,
    smtp_use_tls    INTEGER NOT NULL DEFAULT 1,
    smtp_starttls   INTEGER NOT NULL DEFAULT 0,
    secret_ref      TEXT,                                -- keyring item key (NULL until R5)
    last_synced_at  TEXT,                                -- RFC3339; NULL = never
    sync_status     TEXT    NOT NULL DEFAULT 'idle',     -- 'idle'|'syncing'|'error'
    sync_error      TEXT,                                -- last error message
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE folders (
    id            TEXT    PRIMARY KEY,                   -- uuid
    account_id    TEXT    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,                      -- IMAP path, e.g. "INBOX" or "[Gmail]/All Mail"
    delimiter     TEXT    NOT NULL DEFAULT '/',          -- IMAP hierarchy delimiter
    role          TEXT    NOT NULL DEFAULT 'custom',     -- 'inbox'|'sent'|'drafts'|'archive'|'trash'|'spam'|'all'|'starred'|'custom'
    uid_validity  INTEGER,                               -- IMAP UIDVALIDITY of last sync
    uid_next      INTEGER,                               -- IMAP UIDNEXT seen
    last_seen_uid INTEGER,                               -- highest UID we've fetched
    selectable    INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (account_id, name)
);

CREATE INDEX idx_folders_account_role ON folders(account_id, role);

CREATE TABLE threads (
    id              TEXT    PRIMARY KEY,                 -- uuid
    account_id      TEXT    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    external_id     TEXT,                                -- e.g. Gmail X-GM-THRID
    subject         TEXT,                                -- normalized (Re:/Fwd: stripped)
    last_message_at TEXT,                                -- RFC3339
    message_count   INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (account_id, external_id)
);

CREATE INDEX idx_threads_account_last ON threads(account_id, last_message_at DESC);

-- Each row = one IMAP message in one folder.
CREATE TABLE messages (
    id                TEXT    PRIMARY KEY,                       -- uuid
    account_id        TEXT    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id         TEXT    NOT NULL REFERENCES folders(id)  ON DELETE CASCADE,
    thread_id         TEXT             REFERENCES threads(id)  ON DELETE SET NULL,
    uid               INTEGER NOT NULL,                          -- IMAP UID inside folder
    message_id_header TEXT,                                      -- RFC822 Message-ID
    in_reply_to       TEXT,
    references_header TEXT,                                      -- space-joined list
    from_addr         TEXT    NOT NULL,
    to_addrs          TEXT    NOT NULL DEFAULT '[]',             -- JSON array
    cc_addrs          TEXT    NOT NULL DEFAULT '[]',
    bcc_addrs         TEXT    NOT NULL DEFAULT '[]',
    reply_to          TEXT,
    subject           TEXT,
    snippet           TEXT,                                      -- first ~200 chars of body for list views
    text_body         TEXT,
    html_body         TEXT,
    raw_size          INTEGER NOT NULL DEFAULT 0,                -- bytes
    flags             TEXT    NOT NULL DEFAULT '[]',             -- IMAP flags JSON: ["\\Seen","\\Flagged",...]
    internal_date     TEXT    NOT NULL,                          -- RFC3339, IMAP INTERNALDATE
    sent_at           TEXT,                                      -- RFC3339, Date: header
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (folder_id, uid)
);

CREATE INDEX idx_messages_account_date ON messages(account_id, internal_date DESC);
CREATE INDEX idx_messages_folder_date  ON messages(folder_id,  internal_date DESC);
CREATE INDEX idx_messages_thread       ON messages(thread_id, internal_date);
CREATE INDEX idx_messages_msgid        ON messages(account_id, message_id_header);

-- Attachments live on disk; this table tracks metadata.
CREATE TABLE attachments (
    id            TEXT    PRIMARY KEY,
    message_id    TEXT    NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename      TEXT    NOT NULL,
    content_type  TEXT    NOT NULL,
    content_id    TEXT,                                          -- for inline (cid:) refs
    size_bytes    INTEGER NOT NULL,
    disposition   TEXT    NOT NULL DEFAULT 'attachment',         -- 'inline'|'attachment'
    storage_path  TEXT    NOT NULL,                              -- relative to data_dir/attachments
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_attachments_message ON attachments(message_id);

-- Local drafts. A draft starts local-only; once APPENDed to the server's
-- Drafts folder, remote_uid + remote_folder_id are set.
CREATE TABLE drafts (
    id                TEXT    PRIMARY KEY,
    account_id        TEXT    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    in_reply_to_msg   TEXT             REFERENCES messages(id) ON DELETE SET NULL,
    to_addrs          TEXT    NOT NULL DEFAULT '[]',
    cc_addrs          TEXT    NOT NULL DEFAULT '[]',
    bcc_addrs         TEXT    NOT NULL DEFAULT '[]',
    subject           TEXT,
    text_body         TEXT,
    html_body         TEXT,
    remote_folder_id  TEXT             REFERENCES folders(id)  ON DELETE SET NULL,
    remote_uid        INTEGER,
    created_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_drafts_account ON drafts(account_id, updated_at DESC);

-- MCP gate rules. Default is in postblox.toml; this table is for rules
-- the user adds at runtime via the TUI's "Allow + remember" action.
CREATE TABLE mcp_gates (
    id          TEXT    PRIMARY KEY,
    tool        TEXT    NOT NULL,                                -- e.g. 'send', 'archive'
    arg_pattern TEXT,                                            -- JSON {"to":"*@x.com"}; NULL = matches any
    action      TEXT    NOT NULL,                                -- 'auto_allow'|'require'|'deny'
    note        TEXT,
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_mcp_gates_tool ON mcp_gates(tool);

-- Pending / decided MCP approvals.
CREATE TABLE mcp_approvals (
    id            TEXT    PRIMARY KEY,
    tool          TEXT    NOT NULL,
    args          TEXT    NOT NULL DEFAULT '{}',                 -- JSON
    summary       TEXT    NOT NULL,                              -- one-line human summary
    state         TEXT    NOT NULL DEFAULT 'pending',            -- 'pending'|'allowed'|'denied'|'expired'
    decided_at    TEXT,
    decided_by    TEXT,                                          -- 'user' | 'auto:<rule-id>'
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_mcp_approvals_state ON mcp_approvals(state, created_at DESC);

-- Append-only audit log of MCP-driven actions (and notable user actions).
CREATE TABLE audit_log (
    id          TEXT    PRIMARY KEY,
    actor       TEXT    NOT NULL,                                -- 'user' | 'mcp:<tool>'
    action      TEXT    NOT NULL,
    target      TEXT,                                            -- e.g. message_id
    details     TEXT    NOT NULL DEFAULT '{}',                   -- JSON
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX idx_audit_created ON audit_log(created_at DESC);

-- Full-text search over messages. Triggers keep it in sync. We index a
-- handful of plain-text columns; HTML is stripped to text_body or
-- snippet upstream.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    subject,
    from_addr,
    to_addrs,
    snippet,
    text_body,
    content='messages',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, subject, from_addr, to_addrs, snippet, text_body)
    VALUES (new.rowid, new.subject, new.from_addr, new.to_addrs, new.snippet, new.text_body);
END;

CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addrs, snippet, text_body)
    VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addrs, old.snippet, old.text_body);
END;

CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addrs, snippet, text_body)
    VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addrs, old.snippet, old.text_body);
    INSERT INTO messages_fts(rowid, subject, from_addr, to_addrs, snippet, text_body)
    VALUES (new.rowid, new.subject, new.from_addr, new.to_addrs, new.snippet, new.text_body);
END;
