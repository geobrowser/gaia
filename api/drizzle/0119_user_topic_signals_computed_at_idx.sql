-- GAP-11-2: the geo-db mirror of user_topic_signals reads the rows touched since a point in time
-- (`computed_at >= $1`) after every two-minute sweep. The table has only its primary key
-- (user_id, topic_id, kind), so that read was a full scan. At 32k rows (12 MB, measured
-- 2026-11-06) the cost is small, but it recurs every two minutes and the table grows with users.
-- The index costs a little extra on the sweep's rewrites. No lock concern at this size.
CREATE INDEX IF NOT EXISTS "user_topic_signals_computed_at_idx"
  ON "personalization"."user_topic_signals" ("computed_at");
