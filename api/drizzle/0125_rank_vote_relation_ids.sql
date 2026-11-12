-- GEO-3591: the ranking-indexer honours deleted rank votes, so a ranker can clear their ballot.
--
-- WHY. A GRC-20 DeleteRelation op carries only the relation id. The ranking-indexer stored a ballot's
-- items keyed (ranking, entity, space) with no relation id, so it could not tell which item a delete
-- removed, and it ignored deletes altogether: only an edit that CREATES votes for a Rank replaced that
-- Rank's items. An edit that only deletes votes (a ranker clearing their whole ranking) left the old
-- ballot in ranks.ranking_items and in ranks.ranking_scores. Looking the id up in public.relations at
-- delete time does not work: the kg-indexer reads the same edits on its own consumer group and hard-deletes
-- the row, so whichever indexer runs first decides whether the lookup finds anything.
--
-- 1. ranks.ranking_items.relation_id: the RANK_VOTES relation the item came from. A DeleteRelation of that
--    id, authored in the Rank's own space, deletes the item.
-- 2. ranks.rankings.block_relation_id: the RANK_BLOCK relation that links the Rank to its block. A
--    DeleteRelation of that id from the Rank's space unlinks the Rank (block_id NULL).
-- 3. ranks.pending_recomputes: blocks a delete changed that have not been recomputed yet. The delete and
--    this row commit together; the recompute that rescored the block clears the row. A delete edit retried
--    after a failed recompute matches nothing the second time, so without this row the retry would not
--    know which block to rescore.
-- 4. ranks.deleted_rankings: Rank entities deleted by an edit (DeleteEntity, in the Rank's space). The
--    kg-indexer does not apply DeleteEntity, so the Rank's TYPES relation stays in public.relations and the
--    hourly backfill would otherwise register the deleted Rank again. Cleared when an edit creates the Rank
--    again.
--
-- NO FILL HERE. Existing rows get their relation ids from public.relations when the new ranking-indexer
-- starts (Storage::fill_missing_relation_ids, before it consumes anything) and on every hourly
-- backfill_blocks run. Doing it here would hold this migration's ACCESS EXCLUSIVE locks on both tables
-- through two joins against public.relations, because drizzle applies every pending migration in one
-- transaction. ROLLOUT: stop the old ranking-indexer pods before this runs and start the new image after
-- (see the PR, GEO-3591).

ALTER TABLE "ranks"."ranking_items" ADD COLUMN IF NOT EXISTS "relation_id" uuid;
--> statement-breakpoint
ALTER TABLE "ranks"."rankings" ADD COLUMN IF NOT EXISTS "block_relation_id" uuid;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "ranking_items_relation_id_idx" ON "ranks"."ranking_items" ("relation_id")
	WHERE "relation_id" IS NOT NULL;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "rankings_block_relation_id_idx" ON "ranks"."rankings" ("block_relation_id")
	WHERE "block_relation_id" IS NOT NULL;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "ranks"."pending_recomputes" (
	"seq" bigserial PRIMARY KEY NOT NULL,
	"block_id" uuid NOT NULL,
	"queued_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "pending_recomputes_block_id_idx" ON "ranks"."pending_recomputes" ("block_id");
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "ranks"."deleted_rankings" (
	"id" uuid NOT NULL,
	"space_id" uuid NOT NULL,
	"deleted_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "deleted_rankings_id_space_id_pk" PRIMARY KEY ("id", "space_id")
);
