-- Immutable application-defined session configuration (request parameters, policies).
ALTER TABLE sessions ADD COLUMN config_json TEXT NOT NULL DEFAULT 'null';

CREATE TRIGGER sessions_config_immutable
BEFORE UPDATE OF config_json ON sessions
WHEN NEW.config_json IS NOT OLD.config_json
BEGIN
    SELECT RAISE(ABORT, 'oort: config_json is immutable');
END;
