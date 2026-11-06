/**
 * geodb-proposal-status-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes the `executed_at` / `unexecutable_at` facts of every proposal that has one, from gaia
 * Postgres, into the `governance` system Space as keyed records (backfillProposalStatus, proposalStatusKeyed.ts), the
 * path the consumer uses. Run it before GEODB_PROPOSAL_STATUS goes on; see geodb/RUNBOOK.md §2c-novies. Required, not
 * optional: `space.governance` retains 7 days, and `unexecutable_at` has no event at all. Rerun it (RUNBOOK) to carry
 * an `unexecutable_at` an operator has set since.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_STATUS=1 are both required (the Job refuses otherwise), as for the
 * consumer. Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readProposalStatusRows} from "./pgSource.js"
import {backfillProposalStatus, proposalStatusKeyedEnabled, proposalStatusOffsetsDir} from "./proposalStatusKeyed.js"

mainKeyedBackfill({
	name: "proposal_status",
	refuse: (config) => (proposalStatusKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_STATUS=1 are both required"),
	offsetsDir: proposalStatusOffsetsDir,
	run: (deps, pg, progress) => backfillProposalStatus(deps, readProposalStatusRows(pg), progress),
})
