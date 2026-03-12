ALTER TABLE messages ADD COLUMN search_vector tsvector
    GENERATED ALWAYS AS (
        to_tsvector('english',
            coalesce(subject, '') || ' ' ||
            coalesce(text_body, '') || ' ' ||
            coalesce(extracted_text, '')
        )
    ) STORED;

CREATE INDEX idx_messages_search ON messages USING GIN(search_vector);
