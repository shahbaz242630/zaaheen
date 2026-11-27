-- Limits (ADR-SEC-057 §3, review s123). Counts only: no enquiry content.
--
-- Enquiries per visitor per UTC day. `visitor` is a daily keyed hash of the
-- visitor's network (src/visitor.ts), never the address itself. Deleted the
-- next day by the sweep.
CREATE TABLE visitor_counts (
  day TEXT NOT NULL,
  visitor TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, visitor)
);

-- Enquiries stored by everyone together per UTC day (kind 'stored'). The
-- nightly alert reads the day before; the sweep then deletes earlier days.
CREATE TABLE daily_counts (
  day TEXT NOT NULL,
  kind TEXT NOT NULL,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, kind)
);
