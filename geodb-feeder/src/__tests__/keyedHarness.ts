/** Shared by the keyed-record tests: a model of the engine's statement state and a deps fixture. */
import {mkdtempSync, readFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import type {KeyedRecordDeps} from "../keyedRecord.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {Store} from "../state.js"
import {SystemSpaces} from "../systemSpaces.js"

export const VOTES_SPACE = "f00dbc83ca744e1a9ee0377967846847"
const SEALER = "5".repeat(64)
export const tmp = () => mkdtempSync(join(tmpdir(), "keyed-"))

/**
 * A model of the engine's statement state for the intents the votes path sends. `add` of a held id keeps its active
 * bit; `set_active` of an unseen id is admitted (K1); `put` (geo-db 9d97069a78, tests/facade/natural_keys.zig)
 * closes the cell's other active statements and creates or reopens its own id as active.
 */
export class ModelStore {
	readonly entities = new Set<string>()
	readonly statements = new Map<string, {active: boolean; entity: string; property: string; value: string}>()
	apply(zon: string): void {
		for (const line of zon.split("\n")) {
			const i = line.trim().replace(/,$/, "")
			let m = /^\.\{ \.create_entity = "([0-9a-f]{32})" \}$/.exec(i)
			if (m) {
				this.entities.add(m[1] as string)
				continue
			}
			m = /^\.\{ \.(add|put) = \.\{ \.id = "([0-9a-f]{32})", \.entity = "([0-9a-f]{32})", \.property = "([0-9a-f]{32})", \.value = (.*) \} \}$/.exec(i)
			if (m) {
				const [, tag, id, entity, property, value] = m as unknown as [string, string, string, string, string, string]
				const held = this.statements.get(id)
				if (held && (held.entity !== entity || held.property !== property || held.value !== value)) {
					throw new Error(`${tag} of a held id with a different body`)
				}
				if (tag === "put") {
					for (const s of this.statements.values()) if (s.entity === entity && s.property === property) s.active = false
					this.statements.set(id, {active: true, entity, property, value})
				} else this.statements.set(id, {active: held ? held.active : true, entity, property, value})
				continue
			}
			m = /^\.\{ \.set_active = \.\{ \.statement = "([0-9a-f]{32})", \.active = (true|false) \} \}$/.exec(i)
			if (m) {
				const held = this.statements.get(m[1] as string)
				if (held) held.active = m[2] === "true"
				continue
			}
			if (i.startsWith(".{ .") && !i.startsWith(".{ .space =")) throw new Error(`unmodelled intent ${i}`)
		}
	}
	/** The active values of a record's cell. */
	cell(entity: string, property: string): string[] {
		return [...this.statements.values()].filter((s) => s.active && s.entity === entity && s.property === property).map((s) => s.value)
	}
	snapshot(): string {
		return JSON.stringify([[...this.entities].sort(), [...this.statements].sort(([a], [b]) => a.localeCompare(b))])
	}
}

export function fixture(opts: {offsetsDir?: (dataDir: string) => string; foundFails?: string; foundFailsOnce?: string; applyFailures?: string[]; model?: ModelStore} = {}) {
	let failOnce = opts.foundFailsOnce
	const applyFailures = [...(opts.applyFailures ?? [])]
	const store = new Store(tmp())
	const systemSpaces = new SystemSpaces(store.dir)
	const applied: string[] = []
	const founds: string[] = []
	const published: string[] = []
	const deps: KeyedRecordDeps = {
		db: {
			apply: async (zon: string) => {
				const fail = applyFailures.shift()
				if (fail) return {rc: 1, out: "", err: fail}
				applied.push(zon)
				opts.model?.apply(zon)
				return {rc: 0, out: "", err: ""}
			},
			waitReady: async () => [],
			found: async (_s: string, nonce: string) => {
				founds.push(nonce)
				if (opts.foundFails) throw new Error(opts.foundFails)
				if (failOnce) {
					const e = failOnce
					failOnce = undefined
					throw new Error(e)
				}
				return VOTES_SPACE
			},
		},
		sealer: async () => SEALER,
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		allowPut: true,
		systemSpaces,
		publishSystem: (t) => published.push(t),
		offsets: new ProposalOffsets((opts.offsetsDir ?? ((d) => `${d}/feeder/keyed`))(store.dir)),
	}
	return {store, systemSpaces, applied, founds, published, deps}
}

export const deadLetters = (store: Store): string[] => {
	try {
		return readFileSync(join(store.dir, "errors.jsonl"), "utf8").split("\n").filter(Boolean)
	} catch {
		return []
	}
}

