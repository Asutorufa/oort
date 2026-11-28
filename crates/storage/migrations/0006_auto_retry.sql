-- Per-provider automatic retry of a failed loop (rate limits, transient errors). Off by default.
ALTER TABLE providers ADD COLUMN auto_retry INTEGER NOT NULL DEFAULT 0;
ALTER TABLE providers ADD COLUMN auto_retry_max INTEGER NOT NULL DEFAULT 3;
