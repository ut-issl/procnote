-- Executions of shared templates keep their template as ProcedureTemplate
-- JSON instead of Markdown.
ALTER TABLE executions
ADD COLUMN template_format TEXT NOT NULL DEFAULT 'markdown'
CHECK (template_format IN ('markdown', 'json'));
