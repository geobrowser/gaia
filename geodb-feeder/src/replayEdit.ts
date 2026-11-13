/**
 * replay-edit: re-apply one edit that kg-indexer dropped, through the feeder's own translator (plan 22 section 21,
 * row B1). The operator tool `replay_edit` reads `ipfs_cache` and writes Postgres; this one fetches the payload from an
 * IPFS gateway or a file, translates it exactly as the live consumer does (`translateOps`, the same options), and
 * writes geo-db. It reads and writes no Postgres and needs no Kafka.
 *
 * Dry run by default: it fetches, decodes, translates, runs the safety check and logs counts. `--apply` also writes.
 *
 * Safety rule (the geo-db form of `check_replay_safety`): an edit is REFUSED when any entity it touches already
 * holds an `updatedAt` newer than the edit's own block time. Replaying an old edit over newer state would back-date
 * a stomp on it; `check_replay_safety` makes the same comparison on version keys in Postgres. geo-db keeps no
 * per-property version keys, but the feeder stamps every changed entity's reserved `updatedAt` from the block time
 * (GEODB_ENTITY_TIMES), so the entity is the unit: stricter than Postgres (any later write to the entity refuses,
 * whatever property it touched), never looser. An entity geo-db does not hold is a creation and is safe. An entity it
 * holds with no readable `updatedAt` cannot be shown safe, so it also refuses. An equal time is allowed: that is the
 * edit itself, applied before, and its chunks are idempotent.
 */
import type {ApplyDeps} from "./apply.js"
import {applyIntents} from "./apply.js"
import {UPDATED_AT_PROPERTY, blockInstant} from "./entityTimes.js"
import type {EntityReader} from "./geodbPipe.js"
import {readWithRetry} from "./geodbPipe.js"
import {log} from "./log.js"
import {Overlay} from "./state.js"
import type {Decode} from "./handle.js"
import {type GrcOp, translateOps} from "./translate.js"

export type ReplayEditArgs = {
	uri: string
	/** The gaia Space id, 32 hex digits (dashes accepted). */
	space: string
	block: number
	sequence: number
	/** The edit's block time, unix seconds. Required: the refusal rule compares against it. */
	createdAt: number
	/** Read the payload from this file instead of the gateway. */
	file?: string
	apply: boolean
}

const flagValue = (argv: string[], flag: string): string | undefined => {
	const i = argv.indexOf(flag)
	return i < 0 ? undefined : argv[i + 1]
}

const uintOf = (raw: string | undefined, flag: string): number => {
	if (raw === undefined || !/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw))) throw new Error(`${flag} must be a non-negative integer`)
	return Number(raw)
}

export function parseArgs(argv: string[]): ReplayEditArgs {
	if (argv.some((a) => a.includes("__"))) throw new Error("an argument still holds a manifest placeholder (__X__)")
	const known = new Set(["--uri", "--space", "--block", "--sequence", "--created-at", "--file", "--apply", "--dry-run"])
	if (argv.includes("--apply") && argv.includes("--dry-run")) throw new Error("--apply and --dry-run are exclusive")
	for (const a of argv) if (a.startsWith("--") && !known.has(a)) throw new Error(`unknown flag ${a}`)
	const uri = flagValue(argv, "--uri")
	if (!uri) throw new Error("--uri is required")
	const space = (flagValue(argv, "--space") ?? "").replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(space)) throw new Error("--space must be a space id (32 hex digits, dashes allowed)")
	return {
		uri,
		space,
		block: uintOf(flagValue(argv, "--block"), "--block"),
		sequence: argv.includes("--sequence") ? uintOf(flagValue(argv, "--sequence"), "--sequence") : 0,
		createdAt: uintOf(flagValue(argv, "--created-at"), "--created-at"),
		file: flagValue(argv, "--file"),
		apply: argv.includes("--apply"),
	}
}

export type FetchLike = (url: string, init: {signal: AbortSignal}) => Promise<{ok: boolean; status: number; arrayBuffer(): Promise<ArrayBuffer>}>

/** `ipfs://<cid>[/path]` as a gateway URL; the gateway must be an https URL. */
export function gatewayUrl(uri: string, gateway: string): string {
	const m = /^ipfs:\/\/([A-Za-z0-9]+(?:\/[\w.\-/]*)?)$/.exec(uri)
	if (!m) throw new Error(`--uri must be ipfs://<cid>, not ${uri.slice(0, 80)}`)
	const g = new URL(gateway)
	if (g.protocol !== "https:" || g.username || g.password) throw new Error("the IPFS gateway must be a plain https URL")
	return `${g.origin}${g.pathname.replace(/\/+$/, "")}/${m[1]}`
}

/** The edit's bytes, from `file` when given, else from the gateway. */
export async function loadPayload(
	args: Pick<ReplayEditArgs, "uri" | "file">,
	io: {gateway: string; fetch: FetchLike; readFile: (path: string) => Uint8Array; timeoutMs?: number},
): Promise<Uint8Array> {
	if (args.file) return io.readFile(args.file)
	const res = await io.fetch(gatewayUrl(args.uri, io.gateway), {signal: AbortSignal.timeout(io.timeoutMs ?? 60_000)})
	if (!res.ok) throw new Error(`IPFS gateway answered ${res.status}`)
	return new Uint8Array(await res.arrayBuffer())
}

/** An `updatedAt` row's value as microseconds, or null when it is not a bare integer. */
export function instantOf(value: string): bigint | null {
	return /^\d+$/.test(value) ? BigInt(value) : null
}

export type Conflict = {entity: string; reason: "newer" | "no-updated-at"; heldMicros?: string}

/**
 * Entities of `touched` that make the replay unsafe (see the file comment). One geo-db read per distinct entity, in
 * order; an entity absent from geo-db is not a conflict.
 */
export async function findConflicts(
	reader: EntityReader,
	geoSpace: string,
	touched: readonly string[],
	editMicros: bigint,
	sleep: (ms: number) => Promise<void>,
): Promise<{conflicts: Conflict[]; checked: number; absent: number}> {
	const conflicts: Conflict[] = []
	let absent = 0
	const entities = [...new Set(touched)]
	for (const entity of entities) {
		const rows = await readWithRetry(reader, geoSpace, entity, sleep)
		if (rows.length === 0) {
			absent++
			continue
		}
		const held = rows.filter((r) => r.prop === UPDATED_AT_PROPERTY).map((r) => instantOf(r.value))
		if (held.length === 0 || held.some((h) => h === null)) {
			conflicts.push({entity, reason: "no-updated-at"})
			continue
		}
		const latest = held.reduce((a, b) => ((b as bigint) > (a as bigint) ? b : a)) as bigint
		if (latest > editMicros) conflicts.push({entity, reason: "newer", heldMicros: String(latest)})
	}
	return {conflicts, checked: entities.length, absent}
}

export type ReplayEditDeps = Omit<ApplyDeps, "allowPut"> & {
	decode: Decode
	reader: EntityReader
	/** The store's gaia-space-to-geo-db map; a Space missing here is refused, never founded. */
	spaceRecord: (space: string) => {geo: string | null; nonce: string; keyed: boolean} | undefined
	payload: Uint8Array
	chunk: number
	positions?: boolean
	facets?: boolean
	entityTimes?: boolean
	legacyMap?: boolean
	allowPut?: boolean
}

export type ReplayEditResult =
	| {outcome: "refused"; reason: string; conflicts: Conflict[]; intents: number}
	| {outcome: "dry-run"; intents: number; touched: number; absent: number}
	| {outcome: "applied"; intents: number; applied: number; deadLettered: number}

export async function runReplayEdit(deps: ReplayEditDeps, args: ReplayEditArgs, overlayStore: ConstructorParameters<typeof Overlay>[0]): Promise<ReplayEditResult> {
	// The payload is the bare GRC-20 edit an IPFS URI holds (what kg-indexer's `ipfs_cache.data` holds), not a Hermes envelope.
	const ops: GrcOp[] = (await deps.decode(deps.payload)).ops
	const editMicros = BigInt(blockInstant(args.createdAt) ?? "0")

	const record = deps.spaceRecord(args.space)
	if (!record?.geo) return {outcome: "refused", reason: `space ${args.space} is not mapped in geo-db; this mode founds no Space`, conflicts: [], intents: 0}
	const space = record as typeof record & {geo: string}

	const overlay = new Overlay(overlayStore, args.space)
	const {intents, touched, stats} = translateOps(ops, overlay, {
		positions: deps.positions,
		facets: deps.facets,
		legacyMap: deps.legacyMap,
		blockSeconds: deps.entityTimes ? args.createdAt : undefined,
	})
	const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
	const {conflicts, checked, absent} = await findConflicts(deps.reader, space.geo, touched, editMicros, sleep)
	log("info", "replay-edit plan", {uri: args.uri, block: args.block, ops: ops.length, intents: intents.length, touched: checked, absent, conflicts: conflicts.length, stats})
	if (conflicts.length > 0) {
		return {
			outcome: "refused",
			reason: `${conflicts.length} touched entit${conflicts.length === 1 ? "y" : "ies"} hold a newer or unreadable updatedAt; replaying would stomp later state`,
			conflicts,
			intents: intents.length,
		}
	}
	if (!args.apply) return {outcome: "dry-run", intents: intents.length, touched: checked, absent}

	const meta = {partition: -1, offset: `replay-${args.block}-${args.sequence}`, space: args.space, idempotent: true}
	const r = await applyIntents({...deps, allowPut: deps.allowPut}, space, meta, intents, deps.chunk, 0, () => {})
	overlay.commit()
	return {outcome: "applied", intents: intents.length, applied: r.applied, deadLettered: r.deadLettered}
}
