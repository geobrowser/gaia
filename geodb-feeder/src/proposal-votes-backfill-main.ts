/**
 * geodb-proposal-votes-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `proposal_votes` row of gaia Postgres into the `votes` system Space as a keyed record
 * (backfillProposalVotes, proposalVotesKeyed.ts), the path the proposal-votes consumer uses. Run it before
 * GEODB_PROPOSAL_VOTES goes on; see geodb/RUNBOOK.md §2c-quater. Required, not optional: the stream holds only what
 * `space.governance` still retains.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_VOTES=1 are both required (the Job refuses otherwise), as for the
 * consumer. Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readProposalVoteRows} from "./pgSource.js"
import {backfillProposalVotes, proposalVotesKeyedEnabled, proposalVotesOffsetsDir} from "./proposalVotesKeyed.js"

mainKeyedBackfill({
	name: "proposal_votes",
	refuse: (config) => (proposalVotesKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_VOTES=1 are both required"),
	offsetsDir: proposalVotesOffsetsDir,
	run: (deps, pg, progress) => backfillProposalVotes(deps, readProposalVoteRows(pg), progress),
})
