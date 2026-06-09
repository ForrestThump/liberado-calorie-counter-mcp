-- Direct hash lookup in resolve_user requires an index to be O(1).
CREATE INDEX IF NOT EXISTS users_api_key_hash ON users (api_key_hash);
