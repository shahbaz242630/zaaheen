-- Ask Zaaheen's only stored data (ADR-SEC-052 §3.4a and §3.8).

-- Totals per UTC day: how many questions were asked, answered, turned away,
-- unavailable, or stopped at a limit. Anonymous numbers only.
CREATE TABLE daily_counts (
  day TEXT NOT NULL,
  kind TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, kind)
);

-- Questions per visitor today, for the per-visitor limit. `visitor` is
-- HMAC(secret, day + address), never the address; rows from earlier days are
-- deleted by the daily sweep.
CREATE TABLE visitor_counts (
  day TEXT NOT NULL,
  visitor TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, visitor)
);

-- Questions the box could not answer or turned away, scrubbed, with no
-- identity at all. Deleted after 30 days by the daily sweep.
CREATE TABLE kept_questions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  stored_at INTEGER NOT NULL,
  reason TEXT NOT NULL,
  question TEXT NOT NULL
);
CREATE INDEX kept_questions_stored_at ON kept_questions (stored_at);
