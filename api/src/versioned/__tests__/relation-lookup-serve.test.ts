import type {Id, Op} from "@geoprotocol/grc-20"
import {Effect} from "effect"
import {afterEach, describe, expect, it, vi} from "vitest"

import {extractAffectedEntities, idToUuid, setRelationLookupObserver, setRelationLookupServer} from "../proposal-diff"

const bytes = (n: number) => new Uint8Array(16).fill(n) as unknown as Id
const REL = bytes(0xa1)
const FROM = idToUuid(bytes(0xb2))
const ops = [{type: "deleteRelation", id: REL} as unknown as Op]

const run = (execute: ReturnType<typeof vi.fn>) =>
	// biome-ignore lint/suspicious/noExplicitAny: test mock
	Effect.runPromise(extractAffectedEntities({execute} as any, ops))

afterEach(() => {
	setRelationLookupServer(null)
	setRelationLookupObserver(null)
})

describe("extractAffectedEntities with GEODB_VERSIONS_SERVE", () => {
	it("takes the from-entity from the server and skips both Postgres and the observer", async () => {
		const execute = vi.fn()
		const observer = vi.fn()
		setRelationLookupObserver(observer)
		setRelationLookupServer(async () => new Map([[idToUuid(REL), FROM]]))
		expect([...(await run(execute))]).toEqual([FROM])
		expect(execute).not.toHaveBeenCalled()
		expect(observer).not.toHaveBeenCalled()
	})

	it("asks Postgres and feeds the observer when the server answers null or throws", async () => {
		for (const server of [async () => null, async () => Promise.reject(new Error("boom"))]) {
			const execute = vi.fn().mockResolvedValue({rows: [{id: idToUuid(REL), from_entity_id: FROM}]})
			const observer = vi.fn()
			setRelationLookupObserver(observer)
			setRelationLookupServer(server)
			expect([...(await run(execute))]).toEqual([FROM])
			expect(execute).toHaveBeenCalled()
			expect(observer).toHaveBeenCalledTimes(1)
		}
	})
})
