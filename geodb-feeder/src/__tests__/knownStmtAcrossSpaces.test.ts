import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {Overlay, Store} from "../state.js"
import {translateOps} from "../translate.js"

const id = (n: number) => n.toString(16).padStart(32, "0")
const op = {type: "createRelation", id: id(1), from: id(2), to: id(3), relationType: id(4)}

describe("a relation id created in two Spaces", () => {
	it("is written to the first Space only, as Postgres keeps its row there", () => {
		// kg-indexer's `insert_relations` (storage.rs) is `ON CONFLICT (id) DO UPDATE` without `space_id`: relations.id is a global key
		// and the row stays in the Space that created it. The statement journal is global too, so the feeder agrees.
		const store = new Store(mkdtempSync(join(tmpdir(), "known-stmt-")))
		const a = new Overlay(store, "space-a")
		const first = translateOps([op], a)
		a.commit()
		expect(first.intents.some((i) => i.includes(".add"))).toBe(true)

		const second = translateOps([op], new Overlay(store, "space-b"))
		expect(second.intents.some((i) => i.includes(".add"))).toBe(false)
		expect(second.stats["create-of-known-relation"]).toBe(1)
	})
})
