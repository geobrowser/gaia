/**
 * geodb-proposal-versions-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `proposal_versions` row of gaia Postgres into the `governance` system Space as a keyed record
 * (backfillProposalVersions, proposalVersionsKeyed.ts), the path the proposal-versions consumer uses. Run it before
 * GEODB_PROPOSAL_VERSIONS goes on; see geodb/RUNBOOK.md §2c-octies. Required, not optional: the stream holds only what
 * `space.governance` still retains.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_VERSIONS=1 are both required (the Job refuses otherwise), as for the
 * consumer. Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readProposalVersionRows} from "./pgSource.js"
import {backfillProposalVersions, proposalVersionsKeyedEnabled, proposalVersionsOffsetsDir} from "./proposalVersionsKeyed.js"

mainKeyedBackfill({
	name: "proposal_versions",
	refuse: (config) => (proposalVersionsKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_PROPOSAL_VERSIONS=1 are both required"),
	offsetsDir: proposalVersionsOffsetsDir,
	run: (deps, pg, progress) => backfillProposalVersions(deps, readProposalVersionRows(pg), progress),
})
