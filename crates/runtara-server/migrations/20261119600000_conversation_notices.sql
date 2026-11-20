-- A conversation has something new: said on a channel, by the transaction
-- that stored it (docs/multiple-engines-plan.md, E4). Every engine listens,
-- and wakes its own readers of that conversation: the notice arrives when
-- what it is about can be read, and is not sent if that is rolled back.
-- Postgres sends one notice per conversation per transaction.
CREATE FUNCTION runtara_conversation_said() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('runtara_conversation', NEW.conversation_id::text);
    RETURN NULL;
END
$$;

CREATE TRIGGER conversation_said
    AFTER INSERT ON conversation_events
    FOR EACH ROW EXECUTE FUNCTION runtara_conversation_said();

-- The conversation a run carries, found by the run: an engine that did not
-- start the run asks this.
CREATE INDEX idx_conversations_instance
    ON conversations (tenant_id, instance_id)
    WHERE instance_id IS NOT NULL;
