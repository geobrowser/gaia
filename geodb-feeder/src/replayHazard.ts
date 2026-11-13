/**
 * Which chunks of a replayed edit are unsafe to send again (geo-db ISS-679-318).
 *
 * geo-db applies each single intent idempotently, but not every ordered pair. When an Edit that already applied is
 * sent again under a fresh idempotency key (replay-edit's key is not the live consumer's), six ordered pairs of
 * intents inside one Edit change visible state, because `unset` and `delete_entity` close only the statements active
 * at the Edit's base and a `set_position` names a fixed statement and replacement:
 *
 * | earlier ; later                         | same       | diverges on                          |
 * | --------------------------------------- | ---------- | ------------------------------------ |
 * | put ; unset                             | cell       | fresh-key replay, prefix then whole  |
 * | put ; delete_entity                     | entity     | fresh-key replay, prefix then whole  |
 * | relation_move ; delete_entity           | entity     | fresh-key replay, prefix then whole  |
 * | delete_entity ; relation_move           | entity     | fresh-key replay, prefix then whole  |
 * | relation_retract ; relation_move        | relation   | prefix then whole only               |
 * | relation_move ; relation_move           | relation   | prefix then whole only               |
 *
 * A chunk none of whose intents ever applied is a first apply, and safe. A chunk that applied (wholly, or a bisected
 * part of it) and holds one of the pairs must not be replayed. The prefix-only pairs are refused too: a chunk the
 * live consumer bisected applied a prefix, and the replay's chunk is the whole.
 *
 * Whether an intent applied is read from the feeder's own dead-letter record, `errors.jsonl` on its state volume
 * (`Store.deadLetter`): the edits consumer finishes a message by applying or dead-lettering every intent, so an
 * intent of the edit that is not dead-lettered there applied. An intent the replay translates that the record does
 * not hold (a translation that differs from the original) therefore counts as applied, which errs on refusing.
 * Read-only: this reads the file and writes nothing.
 *
 * Intent forms are the translator's (translate.ts) and putIntent's; the kinds map to geo-db's names as
 * `set_position` = relation_move and `set_active ... .active = false` = relation_retract.
 */

export type HazardKind = "put" | "unset" | "delete_entity" | "relation_move" | "relation_retract"

export type ParsedIntent =
	| {kind: "put"; entity: string; property: string}
	| {kind: "unset"; entity: string; property: string}
	| {kind: "delete_entity"; entity: string}
	| {kind: "relation_move"; entity: string; statement: string; replacement: string}
	| {kind: "relation_retract"; statement: string}
	| {kind: "other"}

const field = (intent: string, name: string): string | undefined => new RegExp(`\\.${name}\\s*=\\s*"([^"]*)"`).exec(intent)?.[1]

/** The intent's kind and the target its pairs compare; `other` for every kind no pair involves. */
export function parseIntent(intent: string): ParsedIntent {
	const head = /^\.\{\s*\.(\w+)\s*=/.exec(intent)?.[1]
	switch (head) {
		case "put":
			return {kind: "put", entity: field(intent, "entity") ?? "", property: field(intent, "property") ?? ""}
		case "unset": {
			// A statement facet unset (`.statement`) names its cell by the statement, as a put of that facet would.
			const entity = field(intent, "entity") ?? field(intent, "statement") ?? ""
			return {kind: "unset", entity, property: field(intent, "property") ?? ""}
		}
		case "delete_entity":
			return {kind: "delete_entity", entity: /^\.\{\s*\.delete_entity\s*=\s*"([^"]*)"/.exec(intent)?.[1] ?? ""}
		case "set_position":
			return {kind: "relation_move", entity: field(intent, "entity") ?? "", statement: field(intent, "id") ?? "", replacement: field(intent, "replacement") ?? ""}
		case "set_active":
			return /\.active\s*=\s*false/.test(intent) ? {kind: "relation_retract", statement: field(intent, "statement") ?? ""} : {kind: "other"}
		default:
			return {kind: "other"}
	}
}

type Scope = "cell" | "entity" | "relation"
export type HazardPair = {earlier: HazardKind; later: HazardKind; same: Scope; prefixOnly: boolean}

/** ISS-679-318's table, in its order. */
export const HAZARD_PAIRS: readonly HazardPair[] = [
	{earlier: "put", later: "unset", same: "cell", prefixOnly: false},
	{earlier: "put", later: "delete_entity", same: "entity", prefixOnly: false},
	{earlier: "relation_move", later: "delete_entity", same: "entity", prefixOnly: false},
	{earlier: "delete_entity", later: "relation_move", same: "entity", prefixOnly: false},
	{earlier: "relation_retract", later: "relation_move", same: "relation", prefixOnly: true},
	{earlier: "relation_move", later: "relation_move", same: "relation", prefixOnly: true},
]

export const pairName = (p: HazardPair): string => `${p.earlier};${p.later}`

const cellOf = (i: ParsedIntent): string | null => (i.kind === "put" || i.kind === "unset" ? `${i.entity}|${i.property}` : null)
const entityOf = (i: ParsedIntent): string | null =>
	i.kind === "put" || i.kind === "unset" || i.kind === "delete_entity" || i.kind === "relation_move" ? i.entity : null
/** A relation's statements: a move names the statement it closes and the replacement it opens. */
const statementsOf = (i: ParsedIntent): string[] =>
	i.kind === "relation_move" ? [i.statement, i.replacement] : i.kind === "relation_retract" ? [i.statement] : []

function sameTarget(scope: Scope, a: ParsedIntent, b: ParsedIntent): boolean {
	if (scope === "cell") {
		const x = cellOf(a)
		return x !== null && x === cellOf(b)
	}
	if (scope === "entity") {
		const x = entityOf(a)
		return x !== null && x !== "" && x === entityOf(b)
	}
	const sb = new Set(statementsOf(b))
	return statementsOf(a).some((s) => s !== "" && sb.has(s))
}

export type PairHit = {pair: string; earlier: number; later: number; prefixOnly: boolean}

/** Every (earlier, later) index pair of `intents` that is one of the six, on the same target. */
export function findHazardPairs(intents: readonly string[]): PairHit[] {
	const parsed = intents.map(parseIntent)
	const hits: PairHit[] = []
	for (let j = 0; j < parsed.length; j++) {
		const b = parsed[j] as ParsedIntent
		if (b.kind === "other") continue
		for (let i = 0; i < j; i++) {
			const a = parsed[i] as ParsedIntent
			if (a.kind === "other") continue
			for (const p of HAZARD_PAIRS) {
				if (a.kind === p.earlier && b.kind === p.later && sameTarget(p.same, a, b)) hits.push({pair: pairName(p), earlier: i, later: j, prefixOnly: p.prefixOnly})
			}
		}
	}
	return hits
}

/**
 * The intents `errors.jsonl` dead-lettered for one edit of one Space, as a multiset (text to count). Lines that
 * are not JSON, or are another edit's, or carry no intent (a decode dead letter), are skipped.
 */
export function deadLetteredIntents(jsonl: string, which: {space: string; edit: string}): Map<string, number> {
	const out = new Map<string, number>()
	for (const line of jsonl.split("\n")) {
		if (!line.trim()) continue
		let r: {space?: unknown; edit?: unknown; intent?: unknown}
		try {
			r = JSON.parse(line)
		} catch {
			continue
		}
		if (r.space !== which.space || r.edit !== which.edit || typeof r.intent !== "string") continue
		out.set(r.intent, (out.get(r.intent) ?? 0) + 1)
	}
	return out
}

export type ChunkPlan = {
	lo: number
	hi: number
	/** `first-apply`: every intent is dead-lettered (never applied). `applied`: at least one is not. */
	status: "first-apply" | "applied"
	deadLettered: number
	pairs: PairHit[]
	/** Set when `--apply` must not send this chunk. */
	refuse: string | null
}

/**
 * Split `intents` as applyIntents does (ranges of `chunk`) and judge each chunk. `deadLetters` is the multiset of the
 * edit's dead-lettered intent texts, or null when no record can be keyed (no `--edit`): then every chunk counts as
 * applied.
 */
export function planChunks(intents: readonly string[], chunk: number, deadLetters: Map<string, number> | null): ChunkPlan[] {
	if (!(chunk > 0)) throw new Error("chunk must be positive")
	const left = new Map(deadLetters ?? [])
	const plans: ChunkPlan[] = []
	for (let lo = 0; lo < intents.length; lo += chunk) {
		const hi = Math.min(lo + chunk, intents.length)
		const slice = intents.slice(lo, hi)
		let dead = 0
		for (const t of slice) {
			const n = left.get(t) ?? 0
			if (n > 0) {
				left.set(t, n - 1)
				dead++
			}
		}
		const status = deadLetters !== null && dead === slice.length ? "first-apply" : "applied"
		// Indices are the edit's, not the chunk's, so a log line points at the intent.
		const pairs = findHazardPairs(slice).map((h) => ({...h, earlier: h.earlier + lo, later: h.later + lo}))
		const refuse =
			status === "applied" && pairs.length > 0
				? `chunk [${lo},${hi}) already applied (${slice.length - dead} of ${slice.length} intents not in the dead-letter record) and holds ${[...new Set(pairs.map((p) => p.pair))].join(", ")} (ISS-679-318): a second apply changes visible state`
				: null
		plans.push({lo, hi, status, deadLettered: dead, pairs, refuse})
	}
	return plans
}

/** The intents of a `zonDoc` document, one per line as zonDoc writes them. */
export function intentsOfZonDoc(doc: string): string[] {
	return doc
		.split("\n")
		.filter((l) => l.startsWith(" .{"))
		.map((l) => l.trim().replace(/,$/, ""))
}

/**
 * Wrap a geo-db `apply` so a document holding an ISS-679-318 pair is never sent: for a replay where no record says
 * which chunks applied, every chunk counts as applied (replay-keyed-main.ts).
 */
export function refusingHazardPairs<A extends (doc: string) => Promise<unknown>>(apply: A): A {
	return (async (doc: string) => {
		const hits = findHazardPairs(intentsOfZonDoc(doc))
		if (hits.length > 0) {
			throw new Error(`refusing to replay a chunk that holds ${[...new Set(hits.map((h) => h.pair))].join(", ")} (ISS-679-318); it may already have applied`)
		}
		return apply(doc)
	}) as A
}
