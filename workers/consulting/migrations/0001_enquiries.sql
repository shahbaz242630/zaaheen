-- AI Readiness Assessment enquiries (ADR-SEC-057). The only personal data
-- this Worker keeps: what the visitor typed into the form, and the two
-- consent answers with the moment they were given. No address, no browser
-- details. Deleted 12 months after it arrived by the daily sweep.
CREATE TABLE enquiries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at INTEGER NOT NULL,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  phone TEXT NOT NULL,
  company TEXT NOT NULL,
  country TEXT NOT NULL,
  size TEXT NOT NULL,
  departments TEXT NOT NULL,
  message TEXT NOT NULL,
  contact_consent INTEGER NOT NULL,
  marketing_consent INTEGER NOT NULL,
  -- 1 once consulting@ was told; 0 if the email failed (read it here instead).
  mailed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX enquiries_received_at ON enquiries (received_at);

-- Emails sent per UTC day: a cap, so a flood of passed checks cannot turn
-- this Worker into a mail cannon. Enquiries over the cap are still stored.
CREATE TABLE daily_mail (
  day TEXT PRIMARY KEY,
  n INTEGER NOT NULL DEFAULT 0
);
