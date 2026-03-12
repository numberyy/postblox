ALTER TABLE messages ADD COLUMN slop_score REAL;
ALTER TABLE messages ADD COLUMN slop_signals JSONB;
ALTER TABLE messages ADD COLUMN category TEXT;
ALTER TABLE messages ADD COLUMN priority TEXT DEFAULT 'normal';
ALTER TABLE messages ADD COLUMN triage_status TEXT DEFAULT 'inbox';
ALTER TABLE messages ADD COLUMN requires_action BOOLEAN DEFAULT false;
