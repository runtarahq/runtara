-- A connection authorized by an OAuth flow started without a browser waits
-- for a member's confirmation (PENDING_CONFIRMATION) before it can be used.
ALTER TABLE connection_data_entity
    DROP CONSTRAINT IF EXISTS connection_data_entity_status_check;
ALTER TABLE connection_data_entity
    ADD CONSTRAINT connection_data_entity_status_check CHECK (status IN (
        'UNKNOWN', 'ACTIVE', 'REQUIRES_RECONNECTION', 'INVALID_CREDENTIALS',
        'PENDING_CONFIRMATION'
    ));
