/**
 * geodb-subspaces-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder scaled
 * to 0. It writes every `subspaces` and `subspace_topics` row of gaia Postgres into the `governance` system Space as
 * an active edge (backfillSubspaces, subspacesKeyed.ts), the path the subspaces consumer uses. Run it before
 * GEODB_SUBSPACES goes on; see geodb/RUNBOOK.md §2c-sexies. Required: the stream holds only what
 * `space.trust.extensions` still retains.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_SUBSPACES=1 are both required (the Job refuses otherwise). Needs serve at
 * geo-db 9d97069a78 or later. The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readSubspaceRows} from "./pgSource.js"
import {backfillSubspaces, subspacesKeyedEnabled, subspacesOffsetsDir} from "./subspacesKeyed.js"

mainKeyedBackfill({
	name: "subspaces",
	refuse: (config) => (subspacesKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_SUBSPACES=1 are both required"),
	offsetsDir: subspacesOffsetsDir,
	run: async (deps, pg, progress) => backfillSubspaces(deps, await readSubspaceRows(pg), progress),
})
