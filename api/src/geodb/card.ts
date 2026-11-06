/**
 * Entity-card statements and the diff between the Postgres and geo-db answers.
 *
 * A card is the multiset of one entity's statements in one space: each value (property, value) and
 * each relation (relation type, target). Both stores are reduced to the same canonical key so the two
 * answers compare without caring how either stores them. The mapping and the multiset match are ported
 * from the geo-shadow harness (diff/common.mjs), which diffed 715/715 snapshot cards and 44/44 live
 * cards clean; keep the two in step.
 */

import {createHash} from "node:crypto"

/** `text` carries a string compared as written; `num` a number compared by value. */
export type StatementKind = "text" | "num"

export type CardStatement = {
	/** Property (value) or relation type id, 32 undashed hex digits, conformed for geo-db. */
	prop: string
	kind: StatementKind
	/** The value, or the relation target id, in canonical text form. */
	norm: string
}

export type ValueRow = {
	property_id: string
	text: string | null
	integer: string | number | null
	float: string | number | null
	decimal: string | number | null
	boolean: boolean | null
	datetime: string | null
	date: string | null
	/** kg-indexer text form `lat,lon[,alt]`. Absent where the caller cannot read it (the gateway shadow). */
	point?: string | null
}

export type RelationRow = {
	type_id: string
	to_entity_id: string
	to_space_id: string | null
}

export type CardDiff = {
	onlyPostgres: CardStatement[]
	onlyGeoDb: {prop: string; raw: string}[]
	/** Statements Postgres holds that geo-db's mapping cannot represent (not a mismatch). */
	skipped: number
}

const UUID_V4 = /^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/i

const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

/**
 * The id geo-db holds for a gaia id. Conforming UUIDv4s pass through; any other legacy id is remapped
 * deterministically, exactly as the shadow loader remaps it (emit/lib.mjs conformingId).
 */
export function conformingId(kind: "entity" | "space", legacyId: string): string {
	const id = compact(legacyId)
	if (UUID_V4.test(id)) return id
	const bytes = Buffer.from(
		createHash("sha256").update(`geo:legacy-${kind}:v1\0${id}`).digest("hex").slice(0, 32),
		"hex",
	)
	bytes.writeUInt8((bytes.readUInt8(6) & 0x0f) | 0x40, 6)
	bytes.writeUInt8((bytes.readUInt8(8) & 0x3f) | 0x80, 8)
	return bytes.toString("hex")
}

function normNumber(value: string | number): string {
	const n = Number(value)
	return Number.isFinite(n) ? String(n) : String(value)
}

function instantNorm(iso: string): string {
	const ms = Date.parse(iso)
	const sec = Math.floor(ms / 1000)
	const nanos = (ms - sec * 1000) * 1_000_000
	return `instant:${sec}.${String(nanos).padStart(9, "0")}`
}

/** One point component as geo-db renders it: `digits` fractional digits, no trailing zeros, no "-0". */
function pointPart(n: number, digits: number): string {
	const t = n.toFixed(digits)
	const trimmed = t.includes(".") ? t.replace(/0+$/, "").replace(/\.$/, "") : t
	return trimmed === "-0" ? "0" : trimmed
}

/** The kg-indexer's `lat,lon[,alt]` as geo-db's `lon, lat[, alt]` (9 fractional digits of a degree, 6 of a metre), null if malformed. */
export function pointNorm(text: string): string | null {
	const parts = text.split(",").map((p) => Number(p.trim()))
	if (parts.length < 2 || parts.length > 3 || parts.some((n) => !Number.isFinite(n))) return null
	const [lat, lon, alt] = parts as [number, number, number | undefined]
	return `${pointPart(lon, 9)}, ${pointPart(lat, 9)}${alt === undefined ? "" : `, ${pointPart(alt, 6)}`}`
}

/** What geo-db renders a point as: `lon, lat` or `lon, lat, alt`. */
const POINT_SHAPED = /^-?\d+(\.\d+)?, -?\d+(\.\d+)?(, -?\d+(\.\d+)?)?$/

/**
 * A Postgres value row as a canonical statement key, or null when geo-db's mapping cannot carry it (a
 * row of bytes, time, schedule, rect or embedding only, or with more than one column set).
 */
export function valueKey(v: ValueRow): {kind: StatementKind; norm: string} | null {
	const candidates: {kind: StatementKind; norm: string}[] = []
	if (v.text != null) candidates.push({kind: "text", norm: String(v.text)})
	if (v.integer != null) candidates.push({kind: "num", norm: normNumber(v.integer)})
	if (v.boolean != null) candidates.push({kind: "text", norm: v.boolean ? "true" : "false"})
	if (v.datetime != null) candidates.push({kind: "text", norm: instantNorm(v.datetime)})
	if (v.date != null) {
		const m = String(v.date).match(/^(-?\d+)-(\d{2})-(\d{2})Z?$/)
		candidates.push({kind: "text", norm: m ? `date:${Number(m[1])}-${m[2]}-${m[3]}` : String(v.date)})
	}
	if (v.point != null) {
		const norm = pointNorm(v.point)
		if (norm !== null) candidates.push({kind: "text", norm})
	}
	if (v.decimal != null) candidates.push({kind: "num", norm: normNumber(v.decimal)})
	else if (v.float != null) candidates.push({kind: "num", norm: normNumber(v.float)})
	return candidates.length === 1 ? (candidates[0] ?? null) : null
}

/** Postgres rows (values, relations) -> canonical statements, plus how many rows could not be mapped. */
export function statementsFromPostgres(
	values: ValueRow[],
	relations: RelationRow[],
): {statements: CardStatement[]; skipped: number} {
	const statements: CardStatement[] = []
	let skipped = 0
	for (const v of values) {
		const key = valueKey(v)
		if (!key) {
			skipped++
			continue
		}
		statements.push({prop: conformingId("entity", v.property_id), ...key})
	}
	for (const r of relations) {
		// geo-db's live translator conforms every relation target as an entity id and drops toSpaceId.
		statements.push({
			prop: conformingId("entity", r.type_id),
			kind: "text",
			norm: conformingId("entity", r.to_entity_id),
		})
	}
	return {statements, skipped}
}

/** `\t`, `\n` and `\` are escaped in geodb-client's text cells. */
export function unescapeText(s: string): string {
	return s.replace(/\\(t|n|\\)/g, (_, c: string) => (c === "t" ? "\t" : c === "n" ? "\n" : "\\"))
}

/** A card row as geo-db answered it. The two names are present only when the read asked for them. */
export type GeoCardRow = {
	prop: string
	raw: string
	/** The property's display name; null where geo-db printed `-`; absent when names were not asked for. */
	propName?: string | null
	/** The value's target's display name (a reference value only); null for `-`; absent as `propName`. */
	targetName?: string | null
	/** The geo-db Space holding the statement; present only on a multi-Space read (`spaces: true`). */
	space?: string
	/** The entity owning the row; present only on a batch read of several entities' cards (`readCards`). */
	entity?: string
}

export type CardOutputOptions = {
	/** The read asked `.args.names = true`: two more cells follow the row's columns, before the weight. */
	names?: boolean
	/** The read asked `.args.claims = true`: two claim columns come before the names. */
	claims?: boolean
	/** The read named several Spaces: the statement cell is `<space>:<statement>`, and each row carries its `space`. */
	spaces?: boolean
}

/** A names cell: `-` (what geodb-client prints for none) is no name (or a target outside the Space), anything else the name as written. */
const nameCell = (cell: string): string | null => (cell === "-" ? null : unescapeText(cell))

/**
 * geodb-client `query` output for an entity card: one row a line, cells tab-separated, columns
 * (property, value, statement id, entity), then with `claims` the claim property and value, then with
 * `names` the property's name and the target's name, and last the weight the client appends. Keeps only
 * the rows of the entity asked for (a card query also returns the entities its backlinks name).
 *
 * Without options the answer is `{prop, raw}` rows exactly as before. A row too short to hold the names
 * the options promise leaves both names absent, so a caller can tell "not answered" from "none".
 */
export function rowsFromGeoDbOutput(
	output: string,
	entityId: string | null,
	options: CardOutputOptions = {},
): GeoCardRow[] {
	const nameAt = 4 + (options.claims ? 2 : 0)
	return output
		.split("\n")
		.filter((line) => line !== "")
		.map((line) => line.split("\t"))
		.filter((cells) => (entityId === null ? /^[0-9a-f]{32}$/.test(cells[3] ?? "") : cells[3] === entityId))
		.map((cells) => {
			const row: GeoCardRow = {prop: cells[0] ?? "", raw: unescapeText(cells[1] ?? "")}
			// A batch read (`entityId` null) keeps each row's owner, as the rows of several cards share one answer.
			if (entityId === null) row.entity = cells[3] as string
			if (options.spaces) {
				const space = /^([0-9a-f]{32}):/.exec(cells[2] ?? "")?.[1]
				if (!space) throw new Error(`a multi-space row has no space prefix: ${(cells[2] ?? "").slice(0, 40)}`)
				row.space = space
			}
			if (options.names && cells.length >= nameAt + 3) {
				row.propName = nameCell(cells[nameAt] ?? "-")
				row.targetName = nameCell(cells[nameAt + 1] ?? "-")
			}
			return row
		})
}

/** A statement referring to an entity, from its card's backlinks: `from` holds `prop` with the entity as value. */
export type GeoBacklinkRow = GeoCardRow & {from: string}

/**
 * geodb-client `query` output for an entity card read with `.args.backlinks` (no names, no claims): the rows
 * whose value is a reference to `entityId`, each with the referring entity (the row's entity cell). The card's
 * own rows are dropped, except one referring to the entity itself, which is a backlink too.
 */
export function backlinkRowsFromGeoDbOutput(
	output: string,
	entityId: string,
	options: Pick<CardOutputOptions, "spaces"> = {},
): GeoBacklinkRow[] {
	return output
		.split("\n")
		.filter((line) => line !== "")
		.map((line) => line.split("\t"))
		.filter((cells) => cells[1] === entityId && /^[0-9a-f]{32}$/.test(cells[3] ?? ""))
		.map((cells) => {
			const row: GeoBacklinkRow = {prop: cells[0] ?? "", raw: entityId, from: cells[3] as string}
			if (options.spaces) {
				const space = /^([0-9a-f]{32}):/.exec(cells[2] ?? "")?.[1]
				if (!space) throw new Error(`a multi-space row has no space prefix: ${(cells[2] ?? "").slice(0, 40)}`)
				row.space = space
			}
			return row
		})
}

/**
 * Properties other indexers write straight into Postgres and no edit carries, so the geo-db backfill
 * excludes them by design (vote Score, ranking rank position and Aggregated rankings, vote ordinal and weighted). The comparison
 * does not count them as replica gaps. Keep in step with DERIVED_PROPERTIES in geodb-feeder/src/pgAdapter.ts
 * (a test in the feeder pins the two ids); moving derived data into geo-db is Stage 4 work.
 */
const DERIVED_PROPERTY_IDS = [
	"85a4668a-42fa-4f48-8969-c0a9de0c294b", // vote-indexer Score
	"e1ffac9f-beb4-4654-8a50-00e1d6c662fb", // ranking-indexer rank position (value)
	"890deffb-3843-49fa-8269-74000e3dcef6", // ranking-indexer rank position (relation type)
	"67651da6-11a9-4246-9ff4-039aafbe9e43", // ranking-indexer Aggregated rankings (relation type)
	"49ee1b89-1820-4e75-a1ae-38a2dcaad4a5", // vote ordinal
	"103701dd-cabe-4a8e-835b-10345327b647", // vote weighted
]

export const DERIVED_PROPERTIES: ReadonlySet<string> = new Set(DERIVED_PROPERTY_IDS.map(compact))

/**
 * Properties only geo-db holds: the feeder's membership dual write puts a `member` and an `editor` reference on each
 * Space's own entity (feeder-minted ids, deterministic; Postgres keeps membership in its own tables, which no card
 * reads). Keep in step with ROLE_PROPERTY in geodb-feeder/src/membership.ts (a feeder test pins the two ids).
 */
export const MEMBERSHIP_PROPERTIES: ReadonlySet<string> = new Set([
	"dbe3bdf637a5458d9206d737fb104793", // member
	"79c3a498486641bc930f3b973de4b4d3", // editor
])

/**
 * An entity's createdAt and updatedAt as geo-db holds them (geo-db docs/api/10-embedded-and-wire.md section 5.6): two
 * reserved properties the feeder writes from block time, each an instant, per Space. Ids fixed by name; keep in step
 * with CREATED_AT_PROPERTY / UPDATED_AT_PROPERTY in geodb-feeder/src/entityTimes.ts (a feeder test pins the two ids).
 */
export const CREATED_AT_PROPERTY = "74d9c34835444345969a3c9d70362ea5"
export const UPDATED_AT_PROPERTY = "c5f5b46a774442acb138c3cba948acec"
export const ENTITY_TIME_PROPERTIES: ReadonlySet<string> = new Set([CREATED_AT_PROPERTY, UPDATED_AT_PROPERTY])

const INSTANT_RAW = /^instant:(-?\d+)\.(\d{9})$/

/**
 * The entity's time across the rows read (one or several Spaces): the least createdAt and the greatest updatedAt, in
 * unix seconds, or null where no row holds one. A row that is not an instant is ignored.
 */
export function entityTimesOf(rows: GeoCardRow[]): {createdAt: number | null; updatedAt: number | null} {
	let createdAt: number | null = null
	let updatedAt: number | null = null
	for (const row of rows) {
		if (!ENTITY_TIME_PROPERTIES.has(row.prop)) continue
		const m = INSTANT_RAW.exec(row.raw)
		if (!m) continue
		const seconds = Number(m[1])
		if (row.prop === CREATED_AT_PROPERTY) createdAt = createdAt === null ? seconds : Math.min(createdAt, seconds)
		else updatedAt = updatedAt === null ? seconds : Math.max(updatedAt, seconds)
	}
	return {createdAt, updatedAt}
}

/** Match Postgres statements against geo-db rows as multisets and return what is unmatched on each side. */
export function diffCards(postgres: {statements: CardStatement[]; skipped: number}, geo: GeoCardRow[]): CardDiff {
	// Derived statements are skipped, not compared: the backfill never writes them.
	const comparable = postgres.statements.filter((s) => !DERIVED_PROPERTIES.has(s.prop))
	const derived = postgres.statements.length - comparable.length
	// Nor are the rows only geo-db holds: derived ones an edit carried (skipped above on the Postgres side), membership, and the entity's times.
	const pool = geo
		.filter(
			(row) =>
				!DERIVED_PROPERTIES.has(row.prop) &&
				!MEMBERSHIP_PROPERTIES.has(row.prop) &&
				!ENTITY_TIME_PROPERTIES.has(row.prop),
		)
		.map((row) => ({...row, used: false}))
	const onlyPostgres: CardStatement[] = []
	// An empty text is no text to a reader, and geo-db has not been seen holding the one Postgres does (ISS: EntityPage
	// b9501592, property 4d9cba1c): a Postgres "" geo-db lacks is skipped, one it holds still matches below.
	let emptyText = 0
	for (const s of comparable) {
		const hit = pool.find(
			(r) => !r.used && r.prop === s.prop && (s.kind === "num" ? normNumber(r.raw) === s.norm : r.raw === s.norm),
		)
		if (hit) hit.used = true
		else if (s.kind === "text" && s.norm === "") emptyText++
		else onlyPostgres.push(s)
	}
	// A Postgres row that could not be mapped may be a point the caller could not read (the gateway shadow), so a
	// point-shaped geo-db row left over is not a gap while one was skipped.
	const unmatched = pool.filter((r) => !r.used && !(postgres.skipped > 0 && POINT_SHAPED.test(r.raw)) && r.raw !== "")
	return {
		onlyPostgres,
		onlyGeoDb: unmatched.map(({prop, raw}) => ({prop, raw})),
		skipped: postgres.skipped + derived + emptyText,
	}
}

export const isEmptyDiff = (d: CardDiff) => d.onlyPostgres.length === 0 && d.onlyGeoDb.length === 0

/** One row of an ordered `entities` page: the entity (geo-db's id), and the property and value of the row. */
export type GeoPageRow = {entity: string; prop: string; raw: string}

/**
 * geodb-client output for an `entities` page: rows a line like a card's (property, value, statement, entity, ..), the
 * entity cell naming the page's object, the page's objects in order. Several Spaces print `<space>:<statement>`; the
 * statement cell is not read. A line without a 32-digit entity cell is refused.
 */
export function pageRowsFromGeoDbOutput(output: string): GeoPageRow[] {
	return output
		.split("\n")
		.filter((line) => line !== "")
		.map((line) => {
			const cells = line.split("\t")
			const entity = cells[3] ?? ""
			if (!/^[0-9a-f]{32}$/.test(entity)) throw new Error(`a page row has no entity cell: ${line.slice(0, 60)}`)
			return {entity, prop: cells[0] ?? "", raw: unescapeText(cells[1] ?? "")}
		})
}

/**
 * An instant row's value as nanoseconds since the epoch (`instant:<seconds>.<nanos>`), or null for any other text.
 */
export function instantNanos(raw: string): bigint | null {
	const m = INSTANT_RAW.exec(raw)
	return m ? BigInt(m[1] as string) * 1_000_000_000n + BigInt(m[2] as string) : null
}
