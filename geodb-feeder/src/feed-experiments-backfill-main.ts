/**
 * geodb-feed-experiments-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `personalization.feed_experiments` row of gaia Postgres into the `config` system Space as
 * keyed records (backfillFeedExperiments, feedExperimentsKeyed.ts). There is no consumer: rerun it after a row is
 * edited. See geodb/RUNBOOK.md §2c-config-experiments.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_FEED_EXPERIMENTS=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {backfillFeedExperiments, feedExperimentsKeyedEnabled, feedExperimentsOffsetsDir} from "./feedExperimentsKeyed.js"
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readFeedExperimentRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "feed_experiments",
	refuse: (config) =>
		feedExperimentsKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_FEED_EXPERIMENTS=1 are both required",
	offsetsDir: feedExperimentsOffsetsDir,
	run: (deps, pg, progress) => backfillFeedExperiments(deps, readFeedExperimentRows(pg), progress),
})
