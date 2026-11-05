/**
 * geodb-votes-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder scaled
 * to 0. It writes every `user_votes` row of gaia Postgres into the `votes` system Space as a keyed record
 * (backfillUserVotes, votesKeyed.ts), the path the votes consumer uses. Run it before GEODB_VOTES goes on; see
 * geodb/RUNBOOK.md §2c-ter. Required, not optional: `curation.votes` keeps 7 days on testnet, so the stream alone
 * cannot rebuild older votes.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_VOTES=1 are both required (the Job refuses otherwise), as for the consumer.
 * Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readUserVoteRows} from "./pgSource.js"
import {backfillUserVotes, votesKeyedEnabled, votesOffsetsDir} from "./votesKeyed.js"

mainKeyedBackfill({
	name: "votes",
	refuse: (config) => (votesKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_VOTES=1 are both required"),
	offsetsDir: votesOffsetsDir,
	run: (deps, pg, progress) => backfillUserVotes(deps, readUserVoteRows(pg), progress),
})
