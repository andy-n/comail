-- Starred mail is a handful of messages in a mailbox of 100k+. The Starred
-- view, its badge and starred tabs build their thread set from this index
-- instead of scanning every message.
CREATE INDEX IF NOT EXISTS idx_messages_starred ON messages(thread_id) WHERE is_starred = 1;
