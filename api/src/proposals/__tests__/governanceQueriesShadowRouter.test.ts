import {Effect} from "effect"
import {Hono} from "hono"
import {describe, expect, it, vi} from "vitest"
import type {QueriesShadow} from "../governance/queriesShadow"
import {createProposalsRouter} from "../router"

const SPACE = "660e8400-e29b-41d4-a716-446655440000"
const TARGET = "770e8400-e29b-41d4-a716-446655440000"

function app(shadow: QueriesShadow | null, execute: ReturnType<typeof vi.fn>) {
	const runtime = {runPromise: <A, E>(e: Effect.Effect<A, E, never>) => Effect.runPromise(e)}
	// biome-ignore lint/suspicious/noExplicitAny: test mock
	const router = createProposalsRouter({execute} as any, runtime as any, null, shadow)
	return new Hono().route("/proposals", router)
}
const spy = (): QueriesShadow & {active: ReturnType<typeof vi.fn>} => ({
	detail: vi.fn(),
	list: vi.fn(),
	active: vi.fn(),
})

describe("queries shadow wiring in the proposals router", () => {
	it.each([
		["members", "member"],
		["editors", "editor"],
	] as const)("hands the %s active check and its served answer to the shadow", async (segment, kind) => {
		const s = spy()
		const res = await app(s, vi.fn().mockResolvedValue({rows: [{exists: true}]})).request(
			`/proposals/space/${SPACE}/${segment}/${TARGET}/active`,
		)
		expect(await res.json()).toEqual({active: true})
		expect(s.active).toHaveBeenCalledWith(kind, TARGET, true)
	})

	it("hands the list and its filters to the shadow, and leaves the response alone", async () => {
		const s = spy()
		const res = await app(s, vi.fn().mockResolvedValue({rows: []})).request(
			`/proposals/space/${SPACE}/status?actionTypes=AddMember`,
		)
		expect(res.status).toBe(200)
		expect(await res.json()).toMatchObject({proposals: []})
		expect(s.list).toHaveBeenCalledWith([], {actionTypes: ["AddMember"], excludeActionTypes: undefined})
	})

	it("serves the same response with no shadow mounted", async () => {
		const res = await app(null, vi.fn().mockResolvedValue({rows: [{exists: false}]})).request(
			`/proposals/space/${SPACE}/members/${TARGET}/active`,
		)
		expect(await res.json()).toEqual({active: false})
	})
})
