/**
 * geodb-space-voting-settings-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the
 * feeder scaled to 0. It writes every `space_voting_settings` row of gaia Postgres into the `governance` system Space as
 * a keyed record (backfillSpaceVotingSettings, spaceVotingSettingsKeyed.ts), the path the consumer uses. Run it before
 * GEODB_SPACE_VOTING_SETTINGS goes on; see geodb/RUNBOOK.md §2c-septies. Required, not optional: the stream holds only
 * what `space.governance` still retains (7 days), and a Space's settings change rarely.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_SPACE_VOTING_SETTINGS=1 are both required (the Job refuses otherwise), as for the
 * consumer. Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readSpaceVotingSettingsRows} from "./pgSource.js"
import {backfillSpaceVotingSettings, spaceVotingSettingsKeyedEnabled, spaceVotingSettingsOffsetsDir} from "./spaceVotingSettingsKeyed.js"

mainKeyedBackfill({
	name: "space_voting_settings",
	refuse: (config) =>
		spaceVotingSettingsKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_SPACE_VOTING_SETTINGS=1 are both required",
	offsetsDir: spaceVotingSettingsOffsetsDir,
	run: (deps, pg, progress) => backfillSpaceVotingSettings(deps, readSpaceVotingSettingsRows(pg), progress),
})
