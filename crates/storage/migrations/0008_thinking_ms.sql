-- How long the model reasoned during a call, so history can show the same duration as live.
ALTER TABLE llm_calls ADD COLUMN thinking_ms INTEGER;
