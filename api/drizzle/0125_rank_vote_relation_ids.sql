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
--    id deletes the item.
-- 2. ranks.rankings.block_relation_id: the RANK_BLOCK relation that links the Rank to its block. A
--    DeleteRelation of that id unlinks the Rank (block_id NULL), so it stops counting in that block.
--
-- Both are nullable. Rows written before this migration are filled below from public.relations, and the
-- hourly backfill_blocks run repeats the same fill, which catches rows an old indexer pod writes during the
-- rollout. An item whose vote relation is already gone from the graph stays NULL: nothing can delete it
-- by id, and the next full re-submission of that ballot replaces it as before.

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
-- RANK_VOTES = 19a4cfff-45f2-4150-abf2-af0f43eb2eec (sdk/src/core/ids.rs). Same statement as
-- Storage::fill_missing_relation_ids.
UPDATE "ranks"."ranking_items" i SET "relation_id" = r."id"
FROM "relations" r
WHERE i."relation_id" IS NULL
	AND r."type_id" = '19a4cfff-45f2-4150-abf2-af0f43eb2eec'
	AND r."from_entity_id" = i."ranking_id"
	AND r."to_entity_id" = i."entity_id"
	AND r."to_space_id" = i."space_id";
--> statement-breakpoint
-- RANK_BLOCK = 09c219c1-03d1-4d2a-a5c7-8edbf2d0182a.
UPDATE "ranks"."rankings" k SET "block_relation_id" = r."id"
FROM "relations" r
WHERE k."block_relation_id" IS NULL
	AND k."block_id" IS NOT NULL
	AND r."type_id" = '09c219c1-03d1-4d2a-a5c7-8edbf2d0182a'
	AND r."from_entity_id" = k."id"
	AND r."to_entity_id" = k."block_id"
	AND r."space_id" = k."space_id";
