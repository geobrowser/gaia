-- GEO-3676: a Ranking Block can have its own row cap and scoring mode, and every scored row says how many
-- ballots include it.
--
-- WHY. The ranking-indexer keeps the top 200 rows of every block and sums each ballot's points. The Best
-- debates block will hold every debate any Debates member has ranked, so 200 rows run out in weeks; and
-- under sums two people's last places tie one person's first, so being on more ballots beats being placed
-- high. Best debates needs up to 2,000 rows and an average of each ballot's place instead. Every other
-- block keeps 200 rows and sums.
--
-- 1. ranks.ranking_scores.ballots: how many eligible ballots include the row (undecayed: an old ballot on
--    a Rolling block still counts as one). Rows written before this migration read 0 until their block is
--    next recomputed. The geo-db mirror (derived collection 11) does not carry it yet.
--
-- 2. ranks.ranking_block_config: one optional row per block id. No row = the default every block has had:
--    200 rows, 'sum'.
--      max_rows      rows the block keeps and publishes (1..10000).
--      scoring_mode  'sum'      each ballot min-max/linearly normalized to [0.5, 1], summed per
--                               (entity, space). Unchanged.
--                    'average'  each ballot scored by position (#1 = 1.0 .. last = 0.5) whatever its rank
--                               type or weights; per entity only (one row however many space ids the
--                               entity was ranked under; it carries the space id most ballots used, ties to
--                               the lowest); score = (sum of points + 0.75) / (ballots + 1); ties to more
--                               ballots, then the lower entity id.
--      applied_max_rows / applied_scoring_mode / applied_at
--                    written by the ranking-indexer after a recompute applied the row. A row whose applied_*
--                    differ from its config (a new row, or an edited one) is recomputed by the hourly
--                    backfill_blocks run, so a config change lands without waiting for a ballot edit.
--
--    Keyed by block id alone: ranks.ranking_blocks is keyed (id, space_id), but the scores and the
--    recompute are per block id.
--
-- APPLYING IT to Best debates once GEO-3685 has created the block (the id is not known yet):
--
--   INSERT INTO ranks.ranking_block_config (block_id, max_rows, scoring_mode)
--   VALUES ('<best debates block id>', 2000, 'average')
--   ON CONFLICT (block_id) DO UPDATE SET max_rows = EXCLUDED.max_rows,
--     scoring_mode = EXCLUDED.scoring_mode, updated_at = now();
--
-- then let the hourly backfill-blocks CronJob pick it up, or run it now:
--   kubectl -n gaia create job --from=cronjob/ranking-indexer-backfill <name>
-- To return a block to the default, set (200, 'sum') rather than deleting the row: a deleted row is not
-- noticed until the block's next ballot edit.

ALTER TABLE "ranks"."ranking_scores" ADD COLUMN IF NOT EXISTS "ballots" integer NOT NULL DEFAULT 0;
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "ranks"."ranking_block_config" (
	"block_id" uuid PRIMARY KEY NOT NULL,
	"max_rows" integer DEFAULT 200 NOT NULL,
	"scoring_mode" text DEFAULT 'sum' NOT NULL,
	"applied_max_rows" integer,
	"applied_scoring_mode" text,
	"applied_at" timestamp with time zone,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "ranking_block_config_max_rows_check" CHECK ("max_rows" BETWEEN 1 AND 10000),
	CONSTRAINT "ranking_block_config_scoring_mode_check" CHECK ("scoring_mode" IN ('sum', 'average'))
);
