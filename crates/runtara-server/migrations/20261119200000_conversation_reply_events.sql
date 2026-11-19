-- What the reply workflow is started for (docs/native-chat-phase-4.md,
-- 4.5): the kinds of event. NULL is a reply and a wait for input.
ALTER TABLE conversations ADD COLUMN reply_events TEXT[];
