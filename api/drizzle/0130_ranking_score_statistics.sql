-- Planner statistics for the ranking tables.
--
-- WHY. entity_ranking_scores and entities each hold about 56M rows. With the default autovacuum settings a table
-- is re-analyzed only after 10% of it changes, about 5.6M rows here, so neither had been analyzed since the stats
-- were last reset. pg_stat_user_tables put both at 2.2M live rows.
--
-- Fresh stats alone did not help the feed. With the default statistics target, the histogram has 100 buckets of
-- about 560k rows each, so the top-scored entities all fall in one bucket. For the score of the 200th-best entity,
-- the planner expected 432,768 rows above it when 203 are. With that estimate, entities_ranked_for_feed's
-- name check hashed all 1.65M names in every parallel worker and spilled them to disk, instead of probing the
-- index 203 times.
--
-- Measured on testnet on 25 Nov, entities_ranked_for_feed(<200th score>, NULL, NULL, NULL, NULL):
--   * default target, fresh ANALYZE: 432,768 rows estimated, 4.2 to 6.2 s, about 200 MB of temp files.
--   * target 10000 on ranking_score: 568 rows estimated, 4.5 ms. Same 203 ids in the same order.
-- That call is the ranked-reads shadow's (rankedReadsShadow.ts). pg_stat_statements put it at 5.5 s mean over
-- 27k calls, and the api pods start it on the same 15-minute timer. Any feed read with a high min_ranking_score
-- benefits the same way.
--
-- Applied by hand on testnet on 25 Nov, followed by ANALYZE public.entity_ranking_scores; this migration records
-- the settings. Every statement is idempotent. Each takes a SHARE UPDATE EXCLUSIVE lock, which does not block
-- reads or writes.
--
-- NO ANALYZE HERE. At target 10000 it samples 3M rows and took 22 s live, too close to the role's 30 s statement
-- timeout for the migrate container. Testnet is already analyzed. A new database gets analyzed by autovacuum.
--
-- AUTOANALYZE AT 1%. Keeps both tables' stats within about 560k changed rows instead of 5.6M.
ALTER TABLE public.entity_ranking_scores ALTER COLUMN ranking_score SET STATISTICS 10000;--> statement-breakpoint
ALTER TABLE public.entity_ranking_scores SET (autovacuum_analyze_scale_factor = 0.01);--> statement-breakpoint
ALTER TABLE public.entities SET (autovacuum_analyze_scale_factor = 0.01);
