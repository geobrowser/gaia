/**
 * geodb-entity-ranking-config-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the
 * feeder scaled to 0. It writes the `entity_ranking_config` row of gaia Postgres into the `config` system Space as one
 * keyed record (backfillEntityRankingConfig, entityRankingConfigKeyed.ts). There is no consumer: rerun it after the row is
 * edited. See geodb/RUNBOOK.md §2c-config-ranking.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_ENTITY_RANKING_CONFIG=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {
	backfillEntityRankingConfig,
	entityRankingConfigKeyedEnabled,
	entityRankingConfigOffsetsDir,
} from "./entityRankingConfigKeyed.js"
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readEntityRankingConfigRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "entity_ranking_config",
	refuse: (config) =>
		entityRankingConfigKeyedEnabled(config)
			? null
			: "GEODB_RECORD_KEYS=1 and GEODB_ENTITY_RANKING_CONFIG=1 are both required",
	offsetsDir: entityRankingConfigOffsetsDir,
	run: (deps, pg, progress) => backfillEntityRankingConfig(deps, readEntityRankingConfigRows(pg), progress),
})
