-- vote_kind: make the three response axes independent
--
-- Adds a `vote_kind` discriminator (0 = curation, 1 = stance, 2 = veracity) so a
-- user can hold one response per (user, object, space, KIND) instead of one
-- response total. Without the widened uniqueness, casting a Verify silently
-- overwrites the same user's upvote — no error, just a vanished vote.
--
-- Everything here lands in ONE migration on purpose. Adding the column without
-- also fixing the consumers below changes `votes_count` from one row per
-- (object_id, object_type, space_id) to up to three, which inflates every SUM()
-- path and makes every direct join return an arbitrary row. Nothing errors;
-- rankings just quietly go wrong. The column and its readers must move together.
--
-- Note on DDL: the uniqueness on `user_votes` and `votes_count` is a UNIQUE
-- CONSTRAINT (see 0018), not a bare unique index. `DROP INDEX` on it fails with
-- "cannot drop index ... because constraint ... requires it", so these are
-- ALTER TABLE ... DROP CONSTRAINT.

-- 1. user_votes: current state, one row per user per object per kind ---------
ALTER TABLE "user_votes" ADD COLUMN "vote_kind" smallint DEFAULT 0 NOT NULL;--> statement-breakpoint

-- The widened key is strictly more permissive than the old one, so no existing
-- row can conflict; every backfilled row is vote_kind = 0 (curation), which is
-- what they all are.
ALTER TABLE "user_votes" DROP CONSTRAINT "user_votes_user_id_object_id_object_type_space_id_unique";--> statement-breakpoint
ALTER TABLE "user_votes" ADD CONSTRAINT "user_votes_user_object_type_space_kind_unique" UNIQUE("user_id","object_id","object_type","space_id","vote_kind");--> statement-breakpoint

-- 2. votes: the raw append-only event log ------------------------------------
--
-- This table stores a decoded direction (up/down/remove), NOT the action hash,
-- so without vote_kind a "remove" row is ambiguous about which axis it cleared
-- and the log stops being replayable into current state. The action hash does
-- not "suffice" here because it is never persisted.
ALTER TABLE "votes" ADD COLUMN "vote_kind" smallint DEFAULT 0 NOT NULL;--> statement-breakpoint

-- 3. votes_count: one aggregate row per object per space per kind ------------
ALTER TABLE "votes_count" ADD COLUMN "vote_kind" smallint DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE "votes_count" DROP CONSTRAINT "votes_count_object_id_object_type_space_id_unique";--> statement-breakpoint
ALTER TABLE "votes_count" ADD CONSTRAINT "votes_count_object_type_space_kind_unique" UNIQUE("object_id","object_type","space_id","vote_kind");--> statement-breakpoint

-- upvotes/downvotes are curation-specific names for what are now generic
-- positive/negative tallies on whichever axis the row belongs to.
ALTER TABLE "votes_count" RENAME COLUMN "upvotes" TO "positive";--> statement-breakpoint
ALTER TABLE "votes_count" RENAME COLUMN "downvotes" TO "negative";--> statement-breakpoint

-- DEPRECATED compatibility shims. `votes_count` is exposed over GraphQL by
-- postgraphile, and the shipped web client's getEntityVoteCount selects
-- `upvotes`/`downvotes` by name — a bare rename breaks the live vote controls
-- the moment this deploys, before the client work lands. These generated
-- columns keep that query working and correct: until the six new actions are
-- registered on chain 55516, only vote_kind = 0 rows exist, so positive ==
-- upvotes for every row in the table.
--
-- Drop both in the follow-up migration that ships alongside the client reading
-- positive/negative. They are read-only: writers must target positive/negative.
ALTER TABLE "votes_count" ADD COLUMN "upvotes" bigint GENERATED ALWAYS AS (positive) STORED;--> statement-breakpoint
ALTER TABLE "votes_count" ADD COLUMN "downvotes" bigint GENERATED ALWAYS AS (negative) STORED;--> statement-breakpoint

-- 4. indexes -----------------------------------------------------------------
--
-- The net-score index is FUNCTIONAL, built on the arithmetic. Renaming the
-- columns alone would leave it indexing the right expression but missing the
-- vote_kind predicate every query now carries, so it would stop matching — a
-- silent performance cliff on ranked surfaces on top of the wrong numbers.
-- vote_kind goes before the ordering expression because queries filter it for
-- equality and then order by net score.
DROP INDEX IF EXISTS "idx_votes_count_space_net_score";--> statement-breakpoint
CREATE INDEX "idx_votes_count_space_net_score" ON "votes_count" ("space_id","vote_kind",("positive" - "negative") DESC) WHERE object_type = 0;--> statement-breakpoint

-- Backs the cross-space aggregation path: WHERE object_type = 0 AND
-- vote_kind = 0 GROUP BY object_id. Leading with vote_kind lets the scan yield
-- object_id already ordered within the kind, so the GroupAggregate keeps
-- avoiding a sort.
DROP INDEX IF EXISTS "idx_votes_count_object_id_entity_only";--> statement-breakpoint
CREATE INDEX "idx_votes_count_object_id_entity_only" ON "votes_count" ("vote_kind","object_id") WHERE object_type = 0;--> statement-breakpoint

-- The notification-indexer's entity-vote poller notifies an entity's creator
-- when its UPVOTES cross a threshold. It must not fire on agrees or
-- verifications, so its keyset index becomes partial on curation to match the
-- poller's predicate exactly. Per-kind notifications, if ever wanted, need
-- their own index.
DROP INDEX IF EXISTS "idx_votes_count_updated_at";--> statement-breakpoint
CREATE INDEX "idx_votes_count_updated_at" ON "votes_count" ("updated_at","id") WHERE object_type = 0 AND vote_kind = 0;--> statement-breakpoint

-- 5. entities_ordered_by_score: scope all four vote paths to curation ---------
--
-- Recreated from 0056 with `vote_kind = 0` added to every votes_count access
-- and the columns renamed. The RAW branches are the ones that matter: the
-- per-space branch LEFT JOINs a single row (which would become an arbitrary one
-- of three), and the cross-space branch SUMs (which would add stance and
-- veracity tallies into the curation score).
CREATE OR REPLACE FUNCTION public.entities_ordered_by_score(
  score_type score_type,
  space_id uuid DEFAULT NULL,
  sort_direction sort_order DEFAULT 'DESC'
)
RETURNS SETOF public.entities
LANGUAGE plpgsql STABLE AS $$
BEGIN
  -- ERRCODE 22023 (invalid_parameter_value) marks these as user input errors so the
  -- API layer can surface the message to clients instead of masking it.
  IF score_type IS NULL THEN
    RAISE EXCEPTION 'score_type is required'
      USING ERRCODE = '22023';
  END IF;

  -- space_id is still required for local scores (they are inherently per-space).
  -- For raw, a NULL space_id means "sum across all spaces".
  IF score_type = 'local' AND space_id IS NULL THEN
    RAISE EXCEPTION 'space_id is required for score_type: %', score_type
      USING ERRCODE = '22023';
  END IF;

  -- Branch on sort_direction explicitly so Postgres can use the ordering indexes
  -- on score / (positive - negative) directly instead of sorting CASE expressions.
  IF score_type = 'local' THEN
    IF sort_direction = 'ASC' THEN
      RETURN QUERY
        SELECT e.*
        FROM entities e
        INNER JOIN local_scores ls ON ls.entity_id = e.id AND ls.space_id = entities_ordered_by_score.space_id
        ORDER BY ls.score ASC, e.id ASC;
    ELSE
      RETURN QUERY
        SELECT e.*
        FROM entities e
        INNER JOIN local_scores ls ON ls.entity_id = e.id AND ls.space_id = entities_ordered_by_score.space_id
        ORDER BY ls.score DESC, e.id ASC;
    END IF;

  ELSIF score_type = 'global' THEN
    IF sort_direction = 'ASC' THEN
      RETURN QUERY
        SELECT e.*
        FROM entities e
        INNER JOIN global_scores gs ON gs.entity_id = e.id
        ORDER BY gs.score ASC, e.id ASC;
    ELSE
      RETURN QUERY
        SELECT e.*
        FROM entities e
        INNER JOIN global_scores gs ON gs.entity_id = e.id
        ORDER BY gs.score DESC, e.id ASC;
    END IF;

  ELSIF score_type = 'raw' AND entities_ordered_by_score.space_id IS NOT NULL THEN
    IF sort_direction = 'ASC' THEN
      RETURN QUERY
        SELECT e.*
        FROM entities e
        LEFT JOIN votes_count vc ON vc.object_id = e.id AND vc.object_type = 0 AND vc.vote_kind = 0 AND vc.space_id = entities_ordered_by_score.space_id
        ORDER BY COALESCE(vc.positive - vc.negative, 0) ASC, e.id ASC;
    ELSE
      RETURN QUERY
        SELECT e.*
        FROM entities e
        LEFT JOIN votes_count vc ON vc.object_id = e.id AND vc.object_type = 0 AND vc.vote_kind = 0 AND vc.space_id = entities_ordered_by_score.space_id
        ORDER BY COALESCE(vc.positive - vc.negative, 0) DESC, e.id ASC;
    END IF;

  ELSIF score_type = 'raw' THEN
    IF sort_direction = 'ASC' THEN
      RETURN QUERY
        SELECT e.*
        FROM entities e
        LEFT JOIN (
          SELECT vc.object_id, SUM(vc.positive - vc.negative)::bigint AS net_score
          FROM votes_count vc
          WHERE vc.object_type = 0 AND vc.vote_kind = 0
          GROUP BY vc.object_id
        ) agg ON agg.object_id = e.id
        ORDER BY COALESCE(agg.net_score, 0) ASC, e.id ASC;
    ELSE
      RETURN QUERY
        SELECT e.*
        FROM entities e
        LEFT JOIN (
          SELECT vc.object_id, SUM(vc.positive - vc.negative)::bigint AS net_score
          FROM votes_count vc
          WHERE vc.object_type = 0 AND vc.vote_kind = 0
          GROUP BY vc.object_id
        ) agg ON agg.object_id = e.id
        ORDER BY COALESCE(agg.net_score, 0) DESC, e.id ASC;
    END IF;
  END IF;
END;
$$;
