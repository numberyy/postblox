ALTER TABLE notification_config
    DROP CONSTRAINT IF EXISTS notification_config_provider_check,
    ADD CONSTRAINT notification_config_provider_check
        CHECK (provider IN ('ntfy', 'email', 'webhook', 'desktop'));
