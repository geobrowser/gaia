-- GEO-2916: `userVotesConnection` hands out an `endCursor` and then answers
-- INTERNAL_SERVER_ERROR when that same cursor is passed back as `after`, so
-- every page after the first fails.
--
-- The server-side error is explicit:
--
--   "The order supplied is not unique, so before/after cursors cannot be used.
--    Please ensure the supplied order includes all the columns from the primary
--    key or a unique constraint."
--
-- PostGraphile makes an ordering unique by appending the table's PRIMARY KEY
-- columns. It does not do this for a bare UNIQUE constraint. `user_votes` was
-- created with `id serial PRIMARY KEY` in 0002, but the v2 table in 0018
-- recreated it with only a UNIQUE constraint — so `VOTED_AT_DESC` has had no
-- tiebreaker, and cursor pagination has been broken on this table ever since.
--
-- Promoting the existing index in place (ADD CONSTRAINT ... PRIMARY KEY USING
-- INDEX) is rejected by Postgres: "index is already associated with a
-- constraint". A unique *index* can be promoted; a unique *constraint's* index
-- cannot. So the constraint is dropped and re-added as the primary key, which
-- does rebuild the index.
--
-- That rebuild is safe here because the table is small: 7,936 rows on testnet at
-- the time of writing, so the ACCESS EXCLUSIVE lock is momentary. If this ever
-- has to be applied to a materially larger `user_votes`, do it as
-- CREATE UNIQUE INDEX CONCURRENTLY followed by
-- DROP CONSTRAINT ... , ADD CONSTRAINT ... PRIMARY KEY USING INDEX <new index>
-- instead, outside a transaction.
--
-- All five columns are already NOT NULL, which a primary key requires.
ALTER TABLE "user_votes"
  DROP CONSTRAINT "user_votes_user_object_type_space_kind_unique";
--> statement-breakpoint
ALTER TABLE "user_votes"
  ADD CONSTRAINT "user_votes_pkey"
  PRIMARY KEY ("user_id", "object_id", "object_type", "space_id", "vote_kind");
