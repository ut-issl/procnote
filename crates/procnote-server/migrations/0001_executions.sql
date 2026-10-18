-- Executions and their append-only event logs.

CREATE TABLE executions (
    id UUID PRIMARY KEY,
    -- Procedure directory name the execution was started from.
    procedure TEXT NOT NULL,
    -- Template source at execution start.
    template_source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per event-log line (an EventRecord as JSON), in order.
-- seq 0 is the LogMeta line.
CREATE TABLE events (
    execution_id UUID NOT NULL REFERENCES executions (id),
    seq BIGINT NOT NULL,
    event_id UUID,
    record JSONB NOT NULL,
    PRIMARY KEY (execution_id, seq)
);

CREATE INDEX events_event_id ON events (execution_id, event_id)
WHERE event_id IS NOT NULL;

-- Stored attachment paths of each execution, and the content they hold.
CREATE TABLE attachments (
    execution_id UUID NOT NULL REFERENCES executions (id),
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (execution_id, path)
);
