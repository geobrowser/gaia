/**
 * geodb-interest-config-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes the `personalization.interest_config` row and every `personalization.interest_signal_weights`
 * row of gaia Postgres into the `config` system Space as keyed records (backfillInterest, interestConfigKeyed.ts).
 * There is no consumer: rerun it after a row is edited. See geodb/RUNBOOK.md §2c-config-interest.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_INTEREST_CONFIG=1 are both required (the Job refuses otherwise). The rest is
 * keyedBackfillJob.ts's.
 */

import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {backfillInterest, interestConfigKeyedEnabled, interestConfigOffsetsDir} from "./interestConfigKeyed.js"
import {readInterestConfigRows, readInterestSignalWeightRows} from "./pgSource.js"

mainKeyedBackfill({
	name: "interest_config",
	refuse: (config) =>
		interestConfigKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_INTEREST_CONFIG=1 are both required",
	offsetsDir: interestConfigOffsetsDir,
	run: (deps, pg, progress) =>
		backfillInterest(deps, {config: readInterestConfigRows(pg), weights: readInterestSignalWeightRows(pg)}, progress),
})
