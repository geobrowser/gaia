-- State of the scoring cronjob's mirror of the score tables into geo-db. One row per mirrored
-- table: `synced` is true when the last run sent every row it meant to, so the next run may send
-- only the rows whose score changed. Missing row or false means the next run rewrites the table.
-- A handful of rows; the cronjob works without the table (it then rewrites everything).
CREATE TABLE IF NOT EXISTS "score_mirror_state" (
  "table_name" text PRIMARY KEY NOT NULL,
  "synced" boolean NOT NULL,
  "updated_at" timestamp with time zone NOT NULL
);
