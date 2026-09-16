-- GEO-2916: `userVotesConnection` hands out an `endCursor` and then answers
-- INTERNAL_SERVER_ERROR when that same cursor is passed back as `after`.
--
-- PostGraphile makes an ordering unique by appending the table's PRIMARY KEY
-- columns, and refuses before/after cursors on an ordering it cannot prove
-- unique. It does not use a bare UNIQUE constraint for that. `user_votes` was
-- created with `id serial PRIMARY KEY` in 0002; the v2 table in 0018 recreated
-- it with only a UNIQUE constraint, so `VOTED_AT_DESC` has had no tiebreaker
-- and cursor pagination has never worked on this table.
--
-- Promoting the existing index in place (ADD CONSTRAINT ... PRIMARY KEY USING
-- INDEX) is rejected by Postgres: "index is already associated with a
-- constraint". A unique *index* can be promoted; a unique *constraint's* index
-- cannot. So the constraint is dropped and re-added as the primary key, which
-- rebuilds the index. Safe here: 7,936 rows on testnet, so the ACCESS EXCLUSIVE
-- lock is momentary. For a materially larger table, use CREATE UNIQUE INDEX
-- CONCURRENTLY then ADD CONSTRAINT ... PRIMARY KEY USING INDEX, outside a
-- transaction. All five columns are already NOT NULL.
--
-- Written idempotently. The first attempt at this migration carried a `when`
-- in _journal.json older than 0085's, and drizzle applies an entry only when
-- `folderMillis > created_at` of the newest applied row — so it was skipped in
-- every environment that already had 0085, silently and without a record, while
-- applying cleanly on a fresh database. Any environment that DID apply the
-- earlier version must be able to run this again harmlessly.
ALTER TABLE "user_votes"
  DROP CONSTRAINT IF EXISTS "user_votes_user_object_type_space_kind_unique";
--> statement-breakpoint
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_constraint
    WHERE conrelid = 'user_votes'::regclass AND contype = 'p'
  ) THEN
    ALTER TABLE "user_votes"
      ADD CONSTRAINT "user_votes_pkey"
      PRIMARY KEY ("user_id", "object_id", "object_type", "space_id", "vote_kind");
  END IF;
END
$$;
