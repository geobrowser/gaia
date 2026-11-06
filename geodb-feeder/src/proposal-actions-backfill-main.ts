/**
 * geodb-proposal-actions-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `proposal_actions` row of gaia Postgres into the `governance` system Space as a keyed record
 * (backfillProposalActions, proposalActionsKeyed.ts), the path the proposal-actions consumer uses. Run it before
 * GEODB_PROPOSAL_ACTIONS goes on; see geodb/RUNBOOK.md §2c-quinquies. Required, not optional: the stream holds only what
 * `space.governance` still retains.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_ACTIONS=1 are both required (the Job refuses otherwise), as for the
 * consumer. Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readProposalActionRows} from "./pgSource.js"
import {backfillProposalActions, proposalActionsKeyedEnabled, proposalActionsOffsetsDir} from "./proposalActionsKeyed.js"

mainKeyedBackfill({
	name: "proposal_actions",
	refuse: (config) => (proposalActionsKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_ACTIONS=1 are both required"),
	offsetsDir: proposalActionsOffsetsDir,
	run: (deps, pg, progress) => backfillProposalActions(deps, readProposalActionRows(pg), progress),
})
