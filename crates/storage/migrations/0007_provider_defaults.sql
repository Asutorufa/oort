-- Provider generation defaults and a provider-neutral thinking level, overridable per preset
-- and per session (the session value can change mid-conversation: it is a request parameter,
-- not part of the cached prefix).
ALTER TABLE providers ADD COLUMN default_reasoning_effort TEXT NOT NULL DEFAULT 'default'
    CHECK (default_reasoning_effort IN ('default', 'off', 'low', 'medium', 'high'));
ALTER TABLE providers ADD COLUMN default_temperature REAL;
ALTER TABLE providers ADD COLUMN default_max_output_tokens INTEGER;
ALTER TABLE providers ADD COLUMN default_context_window INTEGER;
ALTER TABLE providers ADD COLUMN timeout_secs INTEGER NOT NULL DEFAULT 120;

ALTER TABLE presets ADD COLUMN reasoning_effort TEXT NOT NULL DEFAULT 'default'
    CHECK (reasoning_effort IN ('default', 'off', 'low', 'medium', 'high'));

ALTER TABLE sessions ADD COLUMN reasoning_effort TEXT NOT NULL DEFAULT 'default'
    CHECK (reasoning_effort IN ('default', 'off', 'low', 'medium', 'high'));
