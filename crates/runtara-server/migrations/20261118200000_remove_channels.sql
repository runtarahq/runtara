-- The platform-specific chat channels are removed from the server
-- (docs/native-chat.md): a chat platform reaches a workflow as an
-- integration built on the native conversation, not through code of the
-- server's written for it.
--
-- Their triggers go with them. Nothing reads a Channel trigger any more, and
-- its type is no longer one the server knows.
DELETE FROM invocation_trigger WHERE trigger_type = 'CHANNEL';

-- What a channel's webhook left for its conversation to pick up.
DROP TABLE IF EXISTS channel_intake;
