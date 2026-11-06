/**
 * geodb-space-topics-backfill: a one-shot Job (keyedBackfillJob.ts), same image as the feeder, run with the feeder
 * scaled to 0. It writes every `spaces.topic_id` that is set into the `governance` system Space as a keyed record
 * (backfillSpaceTopics, spaceTopicsKeyed.ts), the path the consumer uses. Run it before GEODB_SPACE_TOPICS goes on; see
 * geodb/RUNBOOK.md §2c-undecies. Required, not optional: the stream holds only what `space.topics` still retains
 * (7 days), and a Space's topic changes rarely.
 *
 * Env: GEODB_RECORD_KEYS=1 and GEODB_SPACE_TOPICS=1 are both required (the Job refuses otherwise), as for the consumer.
 * Needs serve at geo-db 9d97069a78 or later (`put`). The rest is keyedBackfillJob.ts's.
 */
import {mainKeyedBackfill} from "./keyedBackfillJob.js"
import {readSpaceTopicRows} from "./pgSource.js"
import {backfillSpaceTopics, spaceTopicsKeyedEnabled, spaceTopicsOffsetsDir} from "./spaceTopicsKeyed.js"

mainKeyedBackfill({
	name: "space_topics",
	refuse: (config) => (spaceTopicsKeyedEnabled(config) ? null : "GEODB_RECORD_KEYS=1 and GEODB_SPACE_TOPICS=1 are both required"),
	offsetsDir: spaceTopicsOffsetsDir,
	run: (deps, pg, progress) => backfillSpaceTopics(deps, readSpaceTopicRows(pg), progress),
})
