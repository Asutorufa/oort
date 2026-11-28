-- Session title auto-generation: who chose the title.
--   default: not decided yet; auto: generated; manual: chosen by the user (never overwritten).
ALTER TABLE sessions ADD COLUMN title_source TEXT NOT NULL DEFAULT 'default'
    CHECK (title_source IN ('default', 'auto', 'manual'));

-- Whether an existing title was typed by the user is unknown: keep all of them.
UPDATE sessions SET title_source = 'manual' WHERE title <> '';
