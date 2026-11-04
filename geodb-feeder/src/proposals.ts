/**
 * Proposal system entities into geo-db. The kg-indexer writes one per proposal straight to Postgres when it
 * handles PROPOSAL_CREATED (kg-indexer/src/handlers/system_entities.rs `map_proposal_created`); no edit carries
 * it, so without this the feeder never sees it. Source: topic `<env>.space.governance`, HermesProposalCreated
 * (event-type header PROPOSAL_CREATED), the same message the kg-indexer applies.
 *
 * The rows are rebuilt exactly as the kg-indexer builds them (same ids, values and relations) and applied
 * through the feeder's own translator, so they equal what the backfill loads from Postgres. They go into the
 * geo-db Space of the proposal's space_id, and only when that Space is already founded: a proposal never
 * founds a Space (that would spend the Space cap on a Space with nothing else in it); the backfill covers
 * Spaces founded later.
 */
import {createHash} from "node:crypto"
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {compactId, entityOps, type PgRelationRow, type PgValueRow} from "./pgAdapter.js"
import {type SpaceDeps} from "./spaces.js"
import {Overlay, writeFileDurable} from "./state.js"
import {translateOps} from "./translate.js"

export type ProposalOutcome = "applied" | "noop" | "ignored" | "unmapped" | "dead_letter"
export type ProposalCreated = {space: string; proposal: string; votingMode: 0 | 1; block: bigint}

// sdk/src/core/ids.rs
const GEO_SYSTEM_NAMESPACE = "ae1e004d-b125-57d8-b3bf-e85a2484f129"
const SYSTEM_TYPES_RELATION_TYPE_ID = "88b3d6ad-288c-529c-a212-0e1c24819185"
const SYSTEM_TYPE_ID = "2ff7ea09-8b9e-50bc-9be7-8a0cafa268d0"
const PROPOSAL_TYPE_ID = "1cb9d5ba-c730-52a7-bc73-1f2c3ff5a330"
const NAME_PROPERTY_ID = "a126ca53-0c8e-48d5-b888-82c734c38935"
const DESCRIPTION_PROPERTY_ID = "9b1f76ff-9711-404c-861e-59dc3fa7d037"
const VOTING_MODE_PROPERTY_ID = "af26ed41-de0f-5b4f-a093-e9d3f683db07"
const SPACE_ID_PROPERTY_ID = "712adc1f-e950-5d14-bbd8-bf2166fe59c1"
const CREATED_AT_BLOCK_PROPERTY_ID = "da2952fb-17d6-53f3-b521-52e254106e0b"

const hex = (v: Uint8Array) => Buffer.from(v).toString("hex")
const hyphenated = (compact: string) =>
	`${compact.slice(0, 8)}-${compact.slice(8, 12)}-${compact.slice(12, 16)}-${compact.slice(16, 20)}-${compact.slice(20)}`

/** RFC 4122 version 5 UUID, as Rust's `Uuid::new_v5`. */
export function uuidV5(namespace: string, name: string): string {
	const h = createHash("sha1").update(Buffer.from(compactId(namespace), "hex")).update(name).digest().subarray(0, 16)
	h[6] = ((h[6] ?? 0) & 0x0f) | 0x50
	h[8] = ((h[8] ?? 0) & 0x3f) | 0x80
	return h.toString("hex")
}

/** The relation row id the kg-indexer gives a system type relation (`derive_system_relation_row_id`). */
export const systemRelationId = (typeName: string, entity: string): string =>
	uuidV5(GEO_SYSTEM_NAMESPACE, `geo:system:rel_id:${typeName}:${hyphenated(compactId(entity))}`)

/**
 * HermesProposalCreated (hermes-schema/proto/governance.proto): space_id = 1, proposal_id = 3, voting_mode = 4
 * (FAST = 0, omitted by proto3; anything but FAST is SLOW, as the kg-indexer reads it), meta = 10
 * (BlockchainMetadata: block_number = 3). Null for any other event type; throws on a malformed message.
 */
export function parseProposalCreated(eventType: string | undefined, payload: Uint8Array): ProposalCreated | null {
	if (eventType !== "PROPOSAL_CREATED") return null
	let space: string | undefined
	let proposal: string | undefined
	let mode = 0n
	let block = 0n
	for (const [f, v] of parseFields(payload)) {
		if (f === 1 && v instanceof Uint8Array) space = hex(v)
		else if (f === 3 && v instanceof Uint8Array) proposal = hex(v)
		else if (f === 4 && typeof v === "bigint") mode = v
		else if (f === 10 && v instanceof Uint8Array) {
			for (const [k, x] of parseFields(v)) if (k === 3 && typeof x === "bigint") block = x
		}
	}
	if (space?.length !== 32 || proposal?.length !== 32) throw new Error("proposal event without a 16-byte space and proposal id")
	return {space, proposal, votingMode: mode === 0n ? 0 : 1, block}
}

/** The Postgres rows `map_proposal_created` writes for this event. */
export function proposalRows(ev: ProposalCreated): {values: PgValueRow[]; relations: PgRelationRow[]} {
	const blank = {language: null, text: null, integer: null, float: null, decimal: null, boolean: null, datetime: null, date: null, point: null}
	const value = (property_id: string, v: Partial<PgValueRow>): PgValueRow => ({entity_id: ev.proposal, property_id, ...blank, ...v})
	const space = hyphenated(ev.space)
	const id = hyphenated(ev.proposal)
	const relation = (name: string, to: string): PgRelationRow => ({
		id: systemRelationId(name, ev.proposal),
		from_entity_id: ev.proposal,
		to_entity_id: to,
		type_id: SYSTEM_TYPES_RELATION_TYPE_ID,
	})
	return {
		values: [
			value(VOTING_MODE_PROPERTY_ID, {integer: String(ev.votingMode)}),
			value(SPACE_ID_PROPERTY_ID, {text: space}),
			value(CREATED_AT_BLOCK_PROPERTY_ID, {integer: ev.block.toString()}),
			value(NAME_PROPERTY_ID, {text: `Proposal ${id}`}),
			value(DESCRIPTION_PROPERTY_ID, {text: `System entity for proposal ${id} in space ${space}`}),
		],
		relations: [relation("System", SYSTEM_TYPE_ID), relation("Proposal", PROPOSAL_TYPE_ID)],
	}
}

export class ProposalOffsets {
	private readonly path: string
	private done: Record<string, number> = {}
	readonly hadOffsets: boolean

	constructor(readonly dir: string) {
		mkdirSync(dir, {recursive: true})
		this.path = `${dir}/offsets.json`
		this.hadOffsets = existsSync(this.path)
		if (this.hadOffsets) this.done = JSON.parse(readFileSync(this.path, "utf8"))
	}
	lastDone(partition: number): number | null {
		return this.done[String(partition)] ?? null
	}
	markDone(partition: number, offset: number): void {
		this.done[String(partition)] = offset
		writeFileDurable(this.path, JSON.stringify(this.done))
	}
}

export type ProposalDeps = ApplyDeps & SpaceDeps

/** Apply one proposal to geo-db. Safe to repeat: the journals say the entity is already held. */
export async function applyProposal(
	deps: ProposalDeps,
	ev: ProposalCreated,
	meta: {partition: number; offset: string},
	chunk = 200,
): Promise<ProposalOutcome> {
	const space = deps.store.spaces[ev.space]
	if (!space?.geo) return "unmapped"
	const entity = cid("entity", ev.proposal)
	if (deps.store.hasEntity(ev.space, entity)) return "noop"
	const {values, relations} = proposalRows(ev)
	const {ops} = entityOps(ev.proposal, values, relations, null, new Set())
	const overlay = new Overlay(deps.store, ev.space)
	const {intents} = translateOps(ops, overlay)
	const r = await applyIntents(deps, space as typeof space & {geo: string}, {...meta, space: ev.space}, intents, chunk, 0, () => {})
	// A refused intent was dead-lettered by applyIntents; leave the journal alone so a rerun plans it again.
	if (r.deadLettered > 0) return "dead_letter"
	overlay.commit()
	return "applied"
}

export async function handleProposalMessage(
	deps: ProposalDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<ProposalOutcome> {
	if (!message.value) return "ignored"
	const header = message.headers?.["event-type"]
	const eventType = header == null ? undefined : Buffer.from(header as Uint8Array | string).toString("utf8")
	let ev: ProposalCreated | null
	try {
		ev = parseProposalCreated(eventType, message.value)
	} catch (e) {
		deps.store.deadLetter({partition, offset: message.offset, topic: "governance", decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}
	if (!ev) return "ignored"
	return applyProposal(deps, ev, {partition, offset: message.offset})
}
