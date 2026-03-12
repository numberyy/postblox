CREATE EXTENSION IF NOT EXISTS vector;

ALTER TABLE messages ADD COLUMN embedding vector(768);

CREATE INDEX idx_messages_embedding ON messages
    USING hnsw (embedding vector_cosine_ops);
