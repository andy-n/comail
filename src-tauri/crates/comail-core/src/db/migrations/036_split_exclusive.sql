-- Per-split "hide from inbox". 1 (the default, and how every split has always
-- behaved) routes matching mail into the split, out of Important. 0 makes the
-- split a view tab: routing skips it and the tab lists inbox threads matching
-- its condition wherever they are routed.
ALTER TABLE split_rules ADD COLUMN exclusive INTEGER NOT NULL DEFAULT 1;
