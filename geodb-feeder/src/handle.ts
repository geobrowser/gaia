import {type ApplyDeps, applyIntents} from "./apply.js"
import {parseHermesEdit} from "./hermes.js"
import {deadLetterSpaceCap, ensureSpace, SpaceCapError, type SpaceDeps} from "./spaces.js"
import {Overlay} from "./state.js"
import {type GrcOp, translateOps} from "./translate.js"

export type Decode = (payload: Uint8Array) => Promise<{ops: GrcOp[]}>
export type HandleDeps = ApplyDeps & SpaceDeps & {decode: Decode; chunk: number; positions?: boolean; entityTimes?: boolean; legacyMap?: boolean}
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
	try {
		const he = parseHermesEdit(message.value)
		if (!he.payload || !he.spaceId) throw new Error("edit has no payload or space")
		spaceId = he.spaceId
		editId = he.id
		blockSeconds = he.createdAt
		ops = (await deps.decode(he.payload)).ops
	} catch (e) {
		deps.store.deadLetter({...meta, decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}

	let space: Awaited<ReturnType<typeof ensureSpace>>
	try {
		space = await ensureSpace(deps, spaceId)
	} catch (e) {
		if (!(e instanceof SpaceCapError)) throw e
		deadLetterSpaceCap(deps, {...meta, edit: editId}, e)
		return "dead_letter"
	}
	const overlay = new Overlay(deps.store, spaceId)
	const {intents, stats} = translateOps(ops, overlay, {
		positions: deps.positions,
		legacyMap: deps.legacyMap,
		blockSeconds: deps.entityTimes ? blockSeconds : undefined,
	})
	const from = Math.min(deps.store.progress(partition, offset), intents.length)
	await applyIntents(
		deps,
		space,
		{...meta, space: spaceId, edit: editId},
		intents,
		deps.chunk,
		from,
		(done) => deps.store.saveProgress(partition, offset, done),
	)
	overlay.commit()
	for (const [k, v] of Object.entries(stats)) deps.metrics.opsDetail.inc({counter: k}, v)
	return intents.length === 0 ? "empty" : "applied"
}
