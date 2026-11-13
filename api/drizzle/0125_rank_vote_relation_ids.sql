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
--    ranks.ranking_items.rank_space_id: the space of the Rank the item belongs to (the space of the edit that
--    wrote it). ranks.rankings is keyed (id, space_id), so the same Rank id can be registered from two spaces;
--    items were keyed by the Rank id alone, so another space registering the id could replace or delete the
--    original's items. Every item read, write and delete in the new indexer is scoped by
--    (ranking_id, rank_space_id). Each registered (id, space) is its own ballot, scored for its own space, as
--    ranks.rankings, dedup (per block and space) and eligibility (per space) already treat it.
--    trending.ranking_member_ballots (0124) is redefined below to join items on the same pair; an item with no
--    rank_space_id yet still matches by Rank id alone, as before this migration.
--
--    The column is NULLABLE here, and the old primary key (ranking_id, entity_id, space_id) is replaced by a
--    unique index on (ranking_id, COALESCE(rank_space_id, nil), entity_id, space_id). An indexer that predates
--    this migration keeps working while the api deploy runs it: its inserts leave rank_space_id NULL and
--    collide exactly as they did under the old key. Setting it NOT NULL here would make those inserts fail
--    (23502, treated as poison: the edit is skipped and its ballot lost). The new indexer sets it on every
--    write and fills NULL rows (Storage::fill_missing_relation_ids, at startup and hourly): from
--    ranks.rankings (the latest row by the dedup order, for an id registered in more than one space), else
--    from the vote relation's own space. A row neither gives a space stays NULL until one does (a Rank not
--    registered yet, or a vote the kg-indexer has not indexed yet); nothing is dropped. A later migration,
--    once no indexer older than this one can run, sets NOT NULL and makes the pair the primary key.
-- 2. ranks.rankings.block_relation_id: the RANK_BLOCK relation that links the Rank to its block. A
--    DeleteRelation of that id from the Rank's space unlinks the Rank (block_id NULL).
-- 3. ranks.pending_recomputes: blocks a delete (or a Rank moving block) changed that have not been
--    recomputed yet. The write and this row commit together; the recompute that rescored the block clears
--    the rows it saw. A delete edit retried after a failed recompute matches nothing the second time, so
--    without this row the retry would not know which block to rescore. `attempts` counts failed recomputes
--    from the live consumer, which stops retrying a block after a few; the hourly backfill keeps retrying
--    it until `dead_after` attempts, then sets `dead_at` and stops (a dead entry is skipped everywhere and
--    logged). Re-enqueue a dead block with:
--      UPDATE ranks.pending_recomputes SET dead_at = NULL, attempts = 0 WHERE block_id = '<block id>';
-- 4. ranks.deleted_rankings: Rank entities deleted by an edit (DeleteEntity), keyed by the deleting edit's
--    space. The kg-indexer does not apply DeleteEntity, so the Rank's TYPES relation stays in
--    public.relations and the hourly backfill would otherwise register the deleted Rank again. The backfill
--    honours a row only when it matches the space of the Rank's TYPES relation, so a delete from another
--    space has no effect. Cleared when an edit creates the Rank again.
--
-- NO FILL HERE: this migration only changes the schema; it updates and deletes no row. Existing rows get
-- their rank_space_id and relation ids when the new ranking-indexer starts (Storage::fill_missing_relation_ids,
-- before it consumes anything) and on every hourly backfill_blocks run. Doing it here would hold this
-- migration's ACCESS EXCLUSIVE locks on both tables through joins against public.relations, because drizzle
-- applies every pending migration in one transaction. ROLLOUT: see the PR (GEO-3591).

ALTER TABLE "ranks"."ranking_items" ADD COLUMN IF NOT EXISTS "relation_id" uuid;
--> statement-breakpoint
ALTER TABLE "ranks"."ranking_items" ADD COLUMN IF NOT EXISTS "rank_space_id" uuid;
--> statement-breakpoint
ALTER TABLE "ranks"."ranking_items" DROP CONSTRAINT IF EXISTS "ranking_items_ranking_id_entity_id_space_id_pk";
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS "ranking_items_rank_space_key" ON "ranks"."ranking_items"
	("ranking_id", (COALESCE("rank_space_id", '00000000-0000-0000-0000-000000000000'::uuid)), "entity_id", "space_id");
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
	"queued_at" timestamp with time zone DEFAULT now() NOT NULL,
	"attempts" integer DEFAULT 0 NOT NULL,
	"dead_at" timestamp with time zone
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
--> statement-breakpoint
-- 0124's member ballots, with items joined on the ballot's own (Rank id, space): a Rank id registered from
-- another space is that space's ballot and must not add its items to this one. An item not attributed to a
-- space yet (written before this migration, and not filled yet) matches by Rank id, as it did before.
CREATE OR REPLACE FUNCTION trending.ranking_member_ballots()
RETURNS TABLE (space_id uuid, block_id uuid, member_space_id uuid, entity_id uuid)
LANGUAGE sql STABLE AS $$
  WITH latest AS (
    SELECT DISTINCT ON (r.block_id, r.space_id) r.id, r.block_id, r.space_id
    FROM ranks.rankings r
    WHERE r.block_id IN (SELECT block_id FROM trending.ranking_place_sources)
    ORDER BY r.block_id, r.space_id, r.updated_at_block DESC, r.update_index DESC, r.id
  )
  SELECT DISTINCT m.space_id, m.block_id, m.member_space_id, ri.entity_id
  FROM latest l
  JOIN trending.ranking_members() m ON m.block_id = l.block_id AND m.member_space_id = l.space_id
  JOIN ranks.ranking_items ri ON ri.ranking_id = l.id
    AND (ri.rank_space_id = l.space_id OR ri.rank_space_id IS NULL)
$$;
