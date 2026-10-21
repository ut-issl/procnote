-- Archived templates are kept (to be restored later) but no longer listed,
-- edited, or executed. Their procedure IDs stay taken.
ALTER TABLE templates ADD COLUMN archived_at TIMESTAMPTZ;
ALTER TABLE templates ADD COLUMN archived_by TEXT;
