/**
 * Space system entities into geo-db. The kg-indexer writes one per registered Space straight to Postgres when it
 * handles a HermesCreateSpace (kg-indexer/src/handlers/system_entities.rs `map_space_registered`); no edit
 * carries it, so without this the feeder never sees it. Source: topic `<env>.space.creations`, the same message
 * the kg-indexer applies (HermesCreateSpace carries no event-type header; SPACE_ADDRESS_UPDATED shares the topic
 * and is ignored, as the kg-indexer only updates the `spaces` row for it, never the entity).
 *
 * The entity lives in the Space it describes (entity id = space id, values and relations in that Space), built
 * with the kg-indexer's ids, values and relations and applied through the feeder's own translator.
 *
 * Policy for a Space registered but not yet founded in geo-db (no edit has reached it): gaia always writes the
 * entity, so ours is not dropped, but it must not found the Space either (that would spend the Space cap on a
 * Space with nothing else in it, as for proposals). The event is parked in a durable `pending` list and applied
 * as soon as the Space is founded, by the periodic sweep. The backfill also loads the entity with the Space
 * (its rows are in the Space's own `space_id`).
 */
import {existsSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {compactId, entityOps, type PgRelationRow, type PgValueRow} from "./pgAdapter.js"
import {
	CREATED_AT_BLOCK_PROPERTY_ID,
	DESCRIPTION_PROPERTY_ID,
	NAME_PROPERTY_ID,
	SYSTEM_TYPE_ID,
	SYSTEM_TYPES_RELATION_TYPE_ID,
	hex,
	hyphenated,
	systemRelationId,
} from "./proposals.js"
import {type SpaceDeps} from "./spaces.js"
import {Overlay, writeFileDurable} from "./state.js"
import {translateOps} from "./translate.js"

export type SpaceEntityOutcome = "applied" | "noop" | "ignored" | "pending" | "dead_letter"
export type SpaceKind = "EoaSpace" | "DaoSpace"
export type SpaceRegistered = {space: string; kind: SpaceKind; address: string; block: bigint}

// sdk/src/core/ids.rs
export const SPACE_TYPE_ID = "f4ce7263-ed14-56c5-aafe-8b74428ce812"
export const EOA_SPACE_TYPE_ID = "09d28123-178f-5828-b85a-81979389c746"
export const DAO_SPACE_TYPE_ID = "afd76215-db11-5cba-81b9-e7f77f865805"
export const SPACE_ADDRESS_PROPERTY_ID = "8f65b58c-d001-5bac-b1d3-3a66ae23193c"

/**
 * HermesCreateSpace (hermes-schema/proto/space.proto): space_id = 1, eoa_space = 3 { owner = 1 },
 * default_dao_space = 4 { address = 1 }, meta = 5 (BlockchainMetadata: block_number = 3).
 * Null for SPACE_ADDRESS_UPDATED and for a message without meta (the kg-indexer writes no entity then);
 * throws on a malformed message or one without a payload (the kg-indexer errors on it too).
 */
export function parseSpaceRegistered(eventType: string | undefined, payload: Uint8Array): SpaceRegistered | null {
	if (eventType === "SPACE_ADDRESS_UPDATED") return null
	let space: string | undefined
	let kind: SpaceKind | undefined
	let address = ""
	let block = 0n
	let meta = false
	for (const [f, v] of parseFields(payload)) {
		if (f === 1 && v instanceof Uint8Array) space = hex(v)
		else if ((f === 3 || f === 4) && v instanceof Uint8Array) {
			kind = f === 3 ? "EoaSpace" : "DaoSpace"
			address = ""
			for (const [k, x] of parseFields(v)) if (k === 1 && x instanceof Uint8Array) address = hex(x)
		} else if (f === 5 && v instanceof Uint8Array) {
			meta = true
			for (const [k, x] of parseFields(v)) if (k === 3 && typeof x === "bigint") block = x
		}
	}
	if (space?.length !== 32) throw new Error("space event without a 16-byte space id")
	if (!kind) throw new Error("space event without a payload")
	if (!meta) return null
	return {space, kind, address, block}
}

/** The Postgres rows `map_space_registered` writes for this event. */
export function spaceRows(ev: SpaceRegistered): {values: PgValueRow[]; relations: PgRelationRow[]} {
	const blank = {language: null, text: null, integer: null, float: null, decimal: null, boolean: null, datetime: null, date: null, point: null}
	const value = (property_id: string, v: Partial<PgValueRow>): PgValueRow => ({entity_id: ev.space, property_id, ...blank, ...v})
	const id = hyphenated(ev.space)
	const relation = (name: string, to: string): PgRelationRow => ({
		id: systemRelationId(name, ev.space),
		from_entity_id: ev.space,
		to_entity_id: to,
		type_id: SYSTEM_TYPES_RELATION_TYPE_ID,
	})
	return {
		values: [
			value(SPACE_ADDRESS_PROPERTY_ID, {text: `0x${ev.address}`}),
			value(CREATED_AT_BLOCK_PROPERTY_ID, {integer: ev.block.toString()}),
			value(NAME_PROPERTY_ID, {text: `Space ${id}`}),
			value(DESCRIPTION_PROPERTY_ID, {text: `System entity for space ${id}`}),
		],
		relations: [
			relation("System", SYSTEM_TYPE_ID),
			relation("Space", SPACE_TYPE_ID),
			relation(ev.kind, ev.kind === "EoaSpace" ? EOA_SPACE_TYPE_ID : DAO_SPACE_TYPE_ID),
		],
	}
}

/**
 * Registered Spaces whose entity waits for the Space to be founded, kept across restarts in `pending.json`
 * (the consumer's offsets are not enough: the message is consumed when it is parked).
 */
export class PendingSpaces {
	private readonly path: string
	private readonly byId = new Map<string, SpaceRegistered>()

	constructor(readonly dir: string) {
		this.path = `${dir}/pending.json`
		if (existsSync(this.path)) {
			for (const e of JSON.parse(readFileSync(this.path, "utf8")) as {space: string; kind: SpaceKind; address: string; block: string}[]) {
				this.byId.set(e.space, {...e, block: BigInt(e.block)})
			}
		}
	}
	get size(): number {
		return this.byId.size
	}
	list(): SpaceRegistered[] {
		return [...this.byId.values()]
	}
	add(ev: SpaceRegistered): void {
		this.byId.set(ev.space, ev)
		this.save()
	}
	remove(space: string): void {
		if (this.byId.delete(space)) this.save()
	}
	private save(): void {
		writeFileDurable(this.path, JSON.stringify(this.list().map((e) => ({...e, block: e.block.toString()}))))
	}
}

export type SpaceEntityDeps = ApplyDeps & SpaceDeps & {pending: PendingSpaces}

/** Apply one Space entity to geo-db. Safe to repeat: the journals say the entity is already held. */
export async function applySpaceEntity(
	deps: SpaceEntityDeps,
	ev: SpaceRegistered,
	meta: {partition: number; offset: string},
	chunk = 200,
): Promise<SpaceEntityOutcome> {
	const space = deps.store.spaces[ev.space]
	if (!space?.geo) {
		deps.pending.add(ev)
		deps.metrics.spaceEntitiesPending.set({}, deps.pending.size)
		return "pending"
	}
	const entity = cid("entity", ev.space)
	if (!deps.store.hasEntity(ev.space, entity)) {
		const {values, relations} = spaceRows(ev)
		const {ops} = entityOps(ev.space, values, relations, null, new Set())
		const overlay = new Overlay(deps.store, ev.space)
		const {intents} = translateOps(ops, overlay)
		const r = await applyIntents(deps, space as typeof space & {geo: string}, {...meta, space: ev.space}, intents, chunk, 0, () => {})
		// A refused intent was dead-lettered by applyIntents; leave the journal alone so a rerun plans it again.
		if (r.deadLettered > 0) return "dead_letter"
		overlay.commit()
		deps.pending.remove(ev.space)
		deps.metrics.spaceEntitiesPending.set({}, deps.pending.size)
		return "applied"
	}
	deps.pending.remove(ev.space)
	deps.metrics.spaceEntitiesPending.set({}, deps.pending.size)
	return "noop"
}

/** Apply the parked entities whose Space has been founded since. Returns how many were applied. */
export async function sweepPending(deps: SpaceEntityDeps): Promise<number> {
	let applied = 0
	for (const ev of deps.pending.list()) {
		if (!deps.store.spaces[ev.space]?.geo) continue
		const outcome = await applySpaceEntity(deps, ev, {partition: -1, offset: "pending"})
		deps.metrics.spaceEntityEvents.inc({outcome})
		if (outcome === "applied") applied++
	}
	return applied
}

export async function handleSpaceCreationMessage(
	deps: SpaceEntityDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<SpaceEntityOutcome> {
	if (!message.value) return "ignored"
	const header = message.headers?.["event-type"]
	const eventType = header == null ? undefined : Buffer.from(header as Uint8Array | string).toString("utf8")
	let ev: SpaceRegistered | null
	try {
		ev = parseSpaceRegistered(eventType, message.value)
	} catch (e) {
		deps.store.deadLetter({partition, offset: message.offset, topic: "space.creations", decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}
	if (!ev) return "ignored"
	return applySpaceEntity(deps, ev, {partition, offset: message.offset})
}
