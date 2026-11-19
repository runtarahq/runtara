-- What a person and a UI need of a conversation
-- (docs/native-chat-phase-4.md, 4.3): a title, and whatever its client keeps
-- about it. Neither means anything to the core.
ALTER TABLE conversations ADD COLUMN title TEXT;
ALTER TABLE conversations ADD COLUMN metadata JSONB;

-- A workflow's conversations, most recently active first.
CREATE INDEX idx_conversations_by_activity
    ON conversations (tenant_id, workflow_id, updated_at DESC, conversation_id);

-- A run's working is kept a shorter time than what was said.
CREATE INDEX idx_conversation_events_activity
    ON conversation_events (created_at) WHERE kind = 'activity';
