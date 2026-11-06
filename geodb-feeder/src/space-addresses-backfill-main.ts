/**
 * geodb-space-addresses-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `spaces.address` into the `governance` system Space as a keyed record
 * (backfillSpaceAddresses, spaceAddressesKeyed.ts), the path the consumer uses. Run it before GEODB_SPACE_ADDRESSES goes on; see
 * geodb/RUNBOOK.md §2c-duodecies. Required, not optional: the stream holds only what `space.creations` still retains.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_SPACE_ADDRESSES=1 are both required (the Job refuses otherwise), as for the consumer.
 * Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readSpaceAddressRows} from "./pgSource.js"
import {backfillSpaceAddresses, spaceAddressesKeyedEnabled, spaceAddressesOffsetsDir} from "./spaceAddressesKeyed.js"

mainKeyedBackfill({
	name: "space_addresses",
	refuse: (config) => (spaceAddressesKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_SPACE_ADDRESSES=1 are both required"),
	offsetsDir: spaceAddressesOffsetsDir,
	run: (deps, pg, progress) => backfillSpaceAddresses(deps, readSpaceAddressRows(pg), progress),
})
