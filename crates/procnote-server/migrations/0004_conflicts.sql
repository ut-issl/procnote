-- Events pushed from devices that no longer applied when they arrived
-- (procnote_app::sync). Kept for a person to deal with, never dropped.
CREATE TABLE conflicts (
    execution_id UUID NOT NULL REFERENCES executions (id),
    event_id UUID NOT NULL,
    -- The pushed log line, as an EventRecord.
    record JSONB NOT NULL,
    reason TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT,
    PRIMARY KEY (execution_id, event_id)
);
