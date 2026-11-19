-- The conversation's events, as a contract (docs/native-chat-phase-4.md, 4.1).
--
-- `turn` is the id of the message an event is for: the one that started the
-- run, or the one that answered what it waited for. A client groups a
-- transcript by it. The conversation keeps the turn it is in.
ALTER TABLE conversation_events ADD COLUMN turn TEXT;
ALTER TABLE conversations ADD COLUMN turn TEXT;

-- A reply that carries an id is said once for its run.
CREATE UNIQUE INDEX idx_conversation_events_reply_once
    ON conversation_events (conversation_id, (body->>'instanceId'), (body->>'replyId'))
    WHERE kind = 'reply' AND body->>'replyId' IS NOT NULL;
