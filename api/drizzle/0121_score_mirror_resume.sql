-- Resume point of the scoring cronjob's mirror into geo-db. When a run is cut off (the per-table
-- time budget) `resume_after` holds the hex key of the last row geo-db answered ok, rows being sent in
-- key order; the next run sends the rows past it, plus earlier rows whose score changed, instead of
-- starting over. NULL with `synced` false means start the table over; NULL with `synced` true means
-- the whole table is in. The cronjob works without the column (it then rewrites an unsynced table).
ALTER TABLE "score_mirror_state" ADD COLUMN IF NOT EXISTS "resume_after" text;
