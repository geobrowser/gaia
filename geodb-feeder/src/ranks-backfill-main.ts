/**
 * geodb-ranks-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder scaled to
 * 0. It writes the rows of gaia's `ranks.ranking_blocks`, `ranks.rankings` and `ranks.ranking_items` from Postgres,
 * read only, into the `votes` system Space as keyed records (backfillRanks, ranksKeyed.ts), the path the consumer
 * uses; items at generation 0, so the stream supersedes them. Run it before GEODB_RANKS goes on; see
 * geodb/RUNBOOK.md §2c-decies. Required, not optional: `knowledge.edits` retains a bounded window, so rankings older
 * than it come only from this snapshot.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_RANKS=1 are both required (the Job refuses otherwise), as for the consumer.
 * Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readRankingBlockRows, readRankingItemRows, readRankingRows} from "./pgSource.js"
import {backfillRanks, ranksKeyedEnabled, ranksOffsetsDir} from "./ranksKeyed.js"

mainKeyedBackfill({
	name: "ranks",
	refuse: (config) => (ranksKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_RANKS=1 are both required"),
	offsetsDir: ranksOffsetsDir,
	run: (deps, pg, progress) =>
		backfillRanks(deps, {blocks: readRankingBlockRows(pg), rankings: readRankingRows(pg), items: readRankingItemRows(pg)}, progress),
})
