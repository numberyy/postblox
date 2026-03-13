CREATE TABLE delivery_status (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    status VARCHAR NOT NULL CHECK (status IN ('delivered', 'bounced', 'complained')),
    bounce_type VARCHAR CHECK (bounce_type IN ('hard', 'soft')),
    details JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_delivery_status_message_id ON delivery_status(message_id);
