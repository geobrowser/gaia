/**
 * geodb-entity-feed-config-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `entity_type_weights`, `entity_type_exclusions` and `entity_feed_blocklist` row of gaia
 * Postgres into the `config` system Space as keyed records (backfillEntityFeedConfig, entityFeedConfigKeyed.ts). This
 * is the operator edit path: edit the row in Postgres, rerun this Job. See geodb/RUNBOOK.md §2c-config-entity-feed.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_ENTITY_FEED_CONFIG=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {backfillEntityFeedConfig, entityFeedConfigKeyedEnabled, entityFeedConfigOffsetsDir} from "./entityFeedConfigKeyed.js"
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readEntityFeedBlockRows, readEntityTypeExclusionRows, readEntityTypeWeightRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "entity_feed_config",
	refuse: (config) =>
		entityFeedConfigKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_ENTITY_FEED_CONFIG=1 are both required",
	offsetsDir: entityFeedConfigOffsetsDir,
	run: (deps, pg, progress) =>
		backfillEntityFeedConfig(
			deps,
			{weights: readEntityTypeWeightRows(pg), exclusions: readEntityTypeExclusionRows(pg), blocks: readEntityFeedBlockRows(pg)},
			progress,
		),
})
