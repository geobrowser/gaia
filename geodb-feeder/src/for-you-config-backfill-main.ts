/**
 * geodb-for-you-config-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes the `personalization.for_you_config` row of gaia Postgres into the `config` system Space as
 * one keyed record (backfillForYouConfig, forYouConfigKeyed.ts). There is no consumer: rerun it after the row is edited.
 * See geodb/RUNBOOK.md §2c-config.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_FOR_YOU_CONFIG=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {backfillForYouConfig, forYouConfigKeyedEnabled, forYouConfigOffsetsDir} from "./forYouConfigKeyed.js"
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readForYouConfigRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "for_you_config",
	refuse: (config) =>
		forYouConfigKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_FOR_YOU_CONFIG=1 are both required",
	offsetsDir: forYouConfigOffsetsDir,
	run: (deps, pg, progress) => backfillForYouConfig(deps, readForYouConfigRows(pg), progress),
})
