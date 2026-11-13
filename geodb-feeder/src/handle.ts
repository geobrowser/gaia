import {type ApplyDeps, applyIntents} from "./apply.js"
import type {AppliedBlockWriter} from "./appliedBlock.js"
import {parseHermesEdit} from "./hermes.js"
import {log} from "./log.js"
import {deadLetterSpaceCap, ensureSpace, SpaceCapError, type SpaceDeps} from "./spaces.js"
import {Overlay} from "./state.js"
import {type GrcOp, translateOps} from "./translate.js"

export type Decode = (payload: Uint8Array) => Promise<{ops: GrcOp[]}>
export type HandleDeps = ApplyDeps & SpaceDeps & {decode: Decode; chunk: number; positions?: boolean; facets?: boolean; entityTimes?: boolean; legacyMap?: boolean; appliedBlock?: Pick<AppliedBlockWriter, "note" | "hold">}
export type Outcome = "applied" | "empty" | "dead_letter"

/**
 * Apply one Kafka message. Safe to call again for the same offset after a crash: translation is
 * deterministic against the journals (which change only at the end), and `store.progress` says how far the
 * previous attempt got.
 */
export async function handleMessage(
	deps: HandleDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null},
): Promise<Outcome> {
	const offset = Number(message.offset)
	const meta = {partition, offset: message.offset}
	if (!message.value) return "empty"

	let spaceId: string
	let ops: GrcOp[]
	let editId: string | undefined
	let blockSeconds: number | null = null
	let blockNumber: number | null = null
	try {
		const he = parseHermesEdit(message.value)
		if (!he.payload || !he.spaceId) throw new Error("edit has no payload or space")
		spaceId = he.spaceId
		editId = he.id
		blockSeconds = he.createdAt
		blockNumber = he.blockNumber
		ops = (await deps.decode(he.payload)).ops
	} catch (e) {
		// The edit is skipped whole: the applied-block row must not claim it (appliedBlock.ts `hold`).
		deps.appliedBlock?.hold(partition, blockNumber)
		deps.store.deadLetter({...meta, decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}

	let space: Awaited<ReturnType<typeof ensureSpace>>
	try {
		space = await ensureSpace(deps, spaceId)
	} catch (e) {
		if (!(e instanceof SpaceCapError)) throw e
		deps.appliedBlock?.hold(partition, blockNumber)
		deadLetterSpaceCap(deps, {...meta, edit: editId}, e)
		return "dead_letter"
	}
	const overlay = new Overlay(deps.store, spaceId)
	const {intents, stats} = translateOps(ops, overlay, {
		positions: deps.positions,
		facets: deps.facets,
		legacyMap: deps.legacyMap,
		blockSeconds: deps.entityTimes ? blockSeconds : undefined,
	})
	const from = Math.min(deps.store.progress(partition, offset), intents.length)
	const {deadLettered} = await applyIntents(
		deps,
		space,
		{...meta, space: spaceId, edit: editId, idempotent: true},
		intents,
		deps.chunk,
		from,
		(done) => deps.store.saveProgress(partition, offset, done),
		// A refused intent is skipped, so the edit is not wholly in the store: hold the row below its block. The
		// `note` below still records the partition's position; the hold caps what the row claims.
		() => deps.appliedBlock?.hold(partition, blockNumber),
	)
	// The overlay's journals say "geo-db holds this". An intent that was dead-lettered never reached geo-db, and the marks
	// are not tied to intents, so an edit with any dead letter commits none: a later translation (a replay, or the next
	// edit touching these entities) then emits what is missing instead of skipping it as known (742e60d9: all 140
	// intents dead-lettered, `connect: Refused`, yet committed). The offset still advances and errors.jsonl is as written.
	// The cost is that an edit's applied entities stay unjournalled: re-emitting a create or add is idempotent per intent.
	// (`deadLettered` counts this attempt only; a crash-resumed message loses earlier attempts' dead letters.)
	if (deadLettered === 0) overlay.commit()
	else log("warn", "overlay not committed: edit has dead-lettered intents", {...meta, space: spaceId, edit: editId, deadLettered, intents: intents.length})
	for (const [k, v] of Object.entries(stats)) deps.metrics.opsDetail.inc({counter: k}, v)
	// Every chunk of the edit is done (applied, or dead-lettered and held above): the applied-block row may now name
	// its block, up to any hold (appliedBlock.ts; best effort).
	await deps.appliedBlock?.note(partition, blockNumber)
	return intents.length === 0 ? "empty" : "applied"
}
