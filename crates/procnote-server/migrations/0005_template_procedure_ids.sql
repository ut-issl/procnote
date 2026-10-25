-- One shared template per procedure ID. The ID is fixed when the template is
-- created; the collab service keeps the document's metadata `id` equal to it.
ALTER TABLE templates ADD COLUMN procedure_id TEXT;

UPDATE templates
SET procedure_id = coalesce(nullif(metadata ->> 'id', ''), 'untitled');

-- Templates created for the same procedure before IDs were unique: the most
-- recently updated one keeps the ID, the others get their own.
UPDATE templates t
SET procedure_id = t.procedure_id || '-' || left(t.id::text, 8)
WHERE EXISTS (
    SELECT 1 FROM templates other
    WHERE other.procedure_id = t.procedure_id
    AND (other.updated_at, other.id) > (t.updated_at, t.id)
);

ALTER TABLE templates ALTER COLUMN procedure_id SET NOT NULL;
ALTER TABLE templates ADD CONSTRAINT templates_procedure_id_key UNIQUE (procedure_id);
