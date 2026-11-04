/**
 * GRC-20 edit ops -> geodb-client `apply` ZON documents. Pure functions: ids go through
 * `conformingId`, and "what geo-db already holds" comes in through the context.
 */
import {cid, hexId} from "./ids.js"
import {languageTag} from "./languages.js"

export type GrcValue = {type: string; language?: string | Uint8Array | null; [k: string]: unknown}
export type GrcPropertyValue = {property: string | Uint8Array; value?: GrcValue}
export type GrcOp = {
	type: string
	id?: string | Uint8Array
	values?: GrcPropertyValue[]
	set?: GrcPropertyValue[]
	unset?: (string | Uint8Array | {property?: string | Uint8Array})[]
	from?: string | Uint8Array
	to?: string | Uint8Array
	relationType?: string | Uint8Array
	position?: unknown
	entity?: unknown
	[k: string]: unknown
}

export type TranslateContext = {
	knownEntity(entity: string): boolean
	markEntity(entity: string): void
	knownStmt(statement: string): boolean
	markStmt(statement: string): void
}

export type Translation = {intents: string[]; touched: string[]; stats: Record<string, number>}

export function zigString(s: string): string {
	let out = '"'
	const clean = String(s).replace(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g, "�")
	for (const ch of clean) {
		const c = ch.codePointAt(0) ?? 0
		if (ch === "\\") out += "\\\\"
		else if (ch === '"') out += '\\"'
		else if (ch === "\n") out += "\\n"
		else if (ch === "\r") out += "\\r"
		else if (ch === "\t") out += "\\t"
		else if (c < 0x20 || c === 0x7f) out += `\\x${c.toString(16).padStart(2, "0")}`
		else out += ch
	}
	return `${out}"`
}

export function normDecimal(coef: bigint | number | string, exp: number | bigint): [bigint, number] {
	let c = BigInt(coef)
	let e = Number(exp)
	if (c === 0n) return [0n, 0]
	while (c % 10n === 0n) {
		c /= 10n
		e += 1
	}
	return [c, e]
}

export function floatToDecimal(x: number): [bigint, number] | null {
	if (!Number.isFinite(x)) return null
	const m = /^(-?)(\d+)(?:\.(\d+))?(?:e([+-]\d+))?$/.exec(String(x))
	if (!m) return null
	const frac = m[3] ?? ""
	return normDecimal(BigInt(`${m[1]}${m[2]}${frac}`), Number(m[4] ?? 0) - frac.length)
}

/** `tag` is a text's BCP 47 primary subtag; `lang` marks a language that has no tag, whose text goes in untagged. */
type Zon = {zon: string; tag?: string; lang?: boolean} | {skip: string}

/**
 * One point component in geo-db's canonical text: rounded to `digits` fractional digits (geo-db keeps
 * nanodegrees and micrometres and refuses a longer fraction), no trailing zeros, no "-0".
 */
function fixed(n: unknown, digits: number): string {
	const t = Number(n).toFixed(digits)
	const trimmed = t.includes(".") ? t.replace(/0+$/, "").replace(/\.$/, "") : t
	return trimmed === "-0" ? "0" : trimmed
}

/** `lon, lat[, alt]` as geo-db's `cast.parsePoint` reads it: 9 fractional digits for degrees, 6 for metres. */
export function pointText(lon: unknown, lat: unknown, alt?: unknown): string {
	return `${fixed(lon, 9)}, ${fixed(lat, 9)}${alt != null ? `, ${fixed(alt, 6)}` : ""}`
}

/** One GRC-20 value -> its ZON value form, or a skip reason. */
export function valueZon(v: GrcValue): Zon {
	switch (v.type) {
		case "text": {
			const zon = `.{ .text = ${zigString(String(v.value))} }`
			if (v.language == null) return {zon}
			const tag = languageTag(v.language)
			return tag ? {zon, tag} : {zon, lang: true}
		}
		case "integer":
			return {zon: `.{ .number = ${String(v.value)} }`}
		case "boolean":
			return {zon: `.{ .boolean = ${v.value ? "true" : "false"} }`}
		case "float": {
			const d = floatToDecimal(Number(v.value))
			return d ? {zon: `.{ .decimal = "${d[0]},${d[1]}" }`} : {skip: "float-nonfinite"}
		}
		case "decimal": {
			const mantissa = v.mantissa as {type: string; value: bigint | number | string}
			if (mantissa.type !== "i64") return {skip: "decimal-bigmantissa"}
			const [c, e] = normDecimal(mantissa.value, v.exponent as number)
			return {zon: `.{ .decimal = "${c},${e}" }`}
		}
		case "datetime": {
			const ms = Date.parse(String(v.value))
			return Number.isNaN(ms) ? {skip: "datetime-unparsable"} : {zon: `.{ .instant = "${ms}000" }`}
		}
		case "date": {
			// "YYYY-MM-DD", "YYYY-MM", "YYYY", each optionally with a trailing Z; precision 0 year, 1 month, 2 day.
			const m = /^(-?\d+)(?:-(\d{2}))?(?:-(\d{2}))?Z?$/.exec(String(v.value))
			if (!m) return {skip: "date-unparsable"}
			const precision = m[3] ? 2 : m[2] ? 1 : 0
			return {zon: `.{ .date = "${precision},${Number(m[1])},${Number(m[2] ?? 0)},${Number(m[3] ?? 0)}" }`}
		}
		case "point":
			return {zon: `.{ .point = "${pointText(v.lon, v.lat, v.alt)}" }`}
		default:
			return {skip: `value-${v.type}`}
	}
}

export function translateOps(ops: GrcOp[], ctx: TranslateContext): Translation {
	const intents: string[] = []
	const touched: string[] = []
	const stats: Record<string, number> = {}
	const bump = (k: string, n = 1) => {
		stats[k] = (stats[k] ?? 0) + n
	}
	const ensure = (e: string) => {
		if (ctx.knownEntity(e)) return false
		ctx.markEntity(e)
		intents.push(`.{ .create_entity = "${e}" }`)
		return true
	}
	const cellKey = (pv: GrcPropertyValue) =>
		`${cid("entity", pv.property)}|${pv.value?.language != null ? hexId(pv.value.language) : ""}`
	const setValues = (e: string, values: GrcPropertyValue[]) => {
		// One op names a cell (entity, property, language) once: the later value wins, as in gaia. geo-db's `set`
		// closes the cell as it stood at the transaction's base, so two sets of one cell in one edit would both
		// stay active (two rows).
		const last = new Map<string, number>()
		values.forEach((pv, i) => {
			last.set(cellKey(pv), i)
		})
		const seen = new Set<string>()
		for (const [i, pv] of values.entries()) {
			if (last.get(cellKey(pv)) !== i) {
				bump("duplicate-value-superseded")
				continue
			}
			const p = cid("entity", pv.property)
			const r = valueZon(pv.value ?? {type: "missing"})
			if ("skip" in r) {
				bump(`skip:${r.skip}`)
				continue
			}
			if (seen.has(p)) bump("multi-value-collapsed")
			seen.add(p)
			if (r.lang) bump("text-language-dropped")
			const language = r.tag ? `, .language = "${r.tag}"` : ""
			intents.push(`.{ .set = .{ .entity = "${e}", .property = "${p}", .value = ${r.zon}${language} } }`)
			if (r.tag) bump("text-language-kept")
			bump("values-set")
		}
	}
	for (const op of ops) {
		switch (op.type) {
			case "createEntity": {
				const e = cid("entity", op.id as string | Uint8Array)
				if (ctx.knownEntity(e)) bump("create-of-known-entity")
				else {
					ctx.markEntity(e)
					intents.push(`.{ .create_entity = "${e}" }`)
				}
				setValues(e, op.values ?? [])
				touched.push(e)
				bump("createEntity")
				break
			}
			case "updateEntity": {
				const e = cid("entity", op.id as string | Uint8Array)
				if (ensure(e)) bump("entity-predates-window")
				setValues(e, op.set ?? [])
				for (const u of op.unset ?? []) {
					const prop = typeof u === "string" || u instanceof Uint8Array ? u : u.property
					if (prop == null) {
						bump("skip:unset-shape")
						continue
					}
					intents.push(`.{ .unset = .{ .entity = "${e}", .property = "${cid("entity", prop)}" } }`)
					bump("unset")
				}
				touched.push(e)
				bump("updateEntity")
				break
			}
			case "createRelation": {
				const from = cid("entity", op.from as string | Uint8Array)
				const to = cid("entity", op.to as string | Uint8Array)
				const sid = cid("statement", op.id as string | Uint8Array)
				ensure(from)
				if (ctx.knownStmt(sid)) {
					bump("create-of-known-relation")
					break
				}
				ctx.markStmt(sid)
				const prop = cid("entity", op.relationType as string | Uint8Array)
				intents.push(
					`.{ .add = .{ .id = "${sid}", .entity = "${from}", .property = "${prop}", .value = .{ .reference = "${to}" } } }`,
				)
				if (op.position != null) bump("relation-position-dropped")
				if (op.entity != null) bump("relation-entity-dropped")
				touched.push(from)
				bump("createRelation")
				break
			}
			case "deleteRelation": {
				const sid = cid("statement", op.id as string | Uint8Array)
				if (!ctx.knownStmt(sid)) {
					bump("deleteRelation-predates-window")
					break
				}
				intents.push(`.{ .set_active = .{ .statement = "${sid}", .active = false } }`)
				bump("deleteRelation")
				break
			}
			case "deleteEntity": {
				const e = cid("entity", op.id as string | Uint8Array)
				if (!ctx.knownEntity(e)) {
					bump("deleteEntity-predates-window")
					break
				}
				intents.push(`.{ .delete_entity = "${e}" }`)
				bump("deleteEntity")
				break
			}
			default:
				bump(`skip:op-${op.type}`)
		}
	}
	return {intents, touched, stats}
}

export function zonDoc(space: string, key: string | null, intents: string[]): string {
	return `.{ .space = "${space}"${key ? `, .key = "${key}"` : ""}, .intents = .{\n${intents.map((i) => ` ${i},`).join("\n")}\n} }\n`
}
