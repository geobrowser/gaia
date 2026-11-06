/**
 * geodb-feed-experiment-members-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `personalization.feed_experiment_members` row of gaia Postgres into the `config` system Space as
 * keyed records (feedExperimentMembersKeyed.ts). There is no consumer: rerun it after a row is
 * edited. See geodb/RUNBOOK.md §2c-config-experiment-members.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_FEED_EXPERIMENT_MEMBERS=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {backfillFeedExperimentMembers, feedExperimentMembersKeyedEnabled, feedExperimentMembersOffsetsDir} from "./feedExperimentMembersKeyed.js"
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readFeedExperimentMemberRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "feed_experiment_members",
	refuse: (config) =>
		feedExperimentMembersKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_FEED_EXPERIMENT_MEMBERS=1 are both required",
	offsetsDir: feedExperimentMembersOffsetsDir,
	run: (deps, pg, progress) => backfillFeedExperimentMembers(deps, readFeedExperimentMemberRows(pg), progress),
})
