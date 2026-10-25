-- 0010_areas_backfill.sql
-- ADR-115 / ADR-SEC-050 (s113) — every AI app sees every area.
--
-- Under the "every area" grant the registry decides what an AI app may read,
-- so an area holding memories but missing from the registry would hide them
-- (a false-empty, ADR-071). Before this migration saves never registered
-- their area: 0008 backfilled once, and a vault that later received memories
-- in a new area (every AI app's `personal`) kept them unregistered. From 0.3.7
-- every memory insert and update registers its area in the same transaction
-- (metadata_store::tx_register_boundary); this catches up the vaults that
-- predate that.
--
-- `personal` is where AI apps save when the user names no area, so it always
-- exists, like `default` (0008). INSERT OR IGNORE keeps user labels and makes
-- this safe to re-run.

INSERT OR IGNORE INTO boundaries (name, description, created_at)
SELECT boundary, NULL, MIN(created_at)
FROM memories
GROUP BY boundary;

INSERT OR IGNORE INTO boundaries (name, description, created_at)
VALUES ('personal', NULL, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
