-- Collaboratively edited templates. The collab service (Hocuspocus) owns
-- `state` and `metadata`; the API server creates rows and lists them.

CREATE TABLE templates (
    id UUID PRIMARY KEY,
    -- Yjs document state (full update), NULL until first stored.
    state BYTEA,
    -- The document's metadata map as JSON (title, id, version, ...), for listing.
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
