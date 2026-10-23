-- 0009_recently_deleted.sql
-- ADR-114 (s109) — "Recently deleted": a memory deleted or rewritten by the
-- user or an AI app is kept here for 30 days so it can be restored.
--
-- One row per deleted memory (`kind = 'deleted'`) or per wording an edit
-- replaced (`kind = 'replaced'`). `memory_json` is the whole memory as it was
-- (vault_core::Memory, serde JSON). No embedding is kept: the memory leaves the
-- search files at once (ADR-039 unchanged) and is re-embedded on restore.
--
-- `by_actor` says who acted: 'user' (the desktop app or CLI, over the keeper's
-- admin connection) or 'app' (an AI app, over MCP). Rows with by_actor = 'app'
-- in the last hour ARE the AI-app change count (ADR-SEC-049), so the limit
-- survives reconnects and restarts.
--
-- `at` is RFC3339 UTC with a fixed width (micros, 'Z'), so text order is time
-- order for the purge and the hourly count.
--
-- Lives in the already-SQLCipher-encrypted vault.db — same posture as every
-- other table, no new crypto path. "Delete everything" / "Delete my account"
-- erase the whole file, this table with it.

CREATE TABLE IF NOT EXISTS recently_deleted (
    entry_id    TEXT PRIMARY KEY,
    memory_id   TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('deleted', 'replaced')),
    by_actor    TEXT NOT NULL CHECK (by_actor IN ('user', 'app')),
    at          TEXT NOT NULL,
    memory_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_recently_deleted_at ON recently_deleted (at);
CREATE INDEX IF NOT EXISTS idx_recently_deleted_actor_at ON recently_deleted (by_actor, at);
