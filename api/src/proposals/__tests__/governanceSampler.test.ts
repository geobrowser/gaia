import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {describe, expect, it, vi} from "vitest"

type Database = NodePgDatabase<Record<string, unknown>>

import {createGovernanceSampler, readGovernanceSamplerConfig, sampleProposalIds} from "../governance/sampler"

const queries = vi.hoisted(() => ({getProposalWithVotes: vi.fn()}))
vi.mock("../queries", async () => {
	const {Effect} = await import("effect")
	return {getProposalWithVotes: (_db: unknown, id: string) => Effect.sync(() => queries.getProposalWithVotes(id))}
})

const idRows = (ids: string[]) =>
	({execute: vi.fn(async () => ({rows: ids.map((id) => ({id}))}))}) as unknown as Database

describe("config", () => {
	it("is off unless the flag says on, and clamps tunables", () => {
		expect(readGovernanceSamplerConfig({}).enabled).toBe(false)
		expect(readGovernanceSamplerConfig({GEODB_GOVERNANCE_SHADOW_SAMPLER: "1"}).enabled).toBe(true)
		const c = readGovernanceSamplerConfig({GEODB_GOVERNANCE_SHADOW_SAMPLER_SAMPLE: "9999"})
		expect(c.sample).toBe(50)
		expect(c.intervalMs).toBe(60_000)
	})
})

describe("sampleProposalIds", () => {
	it("wraps to the start when the pivot is near the end, keeping each id once", async () => {
		const calls: string[][] = [["b"], ["a", "b"]]
		const db = {
			execute: vi.fn(async () => ({rows: (calls.shift() ?? []).map((id) => ({id}))})),
		} as unknown as Database
		expect(await sampleProposalIds(db, "p", 3)).toEqual(["b", "a"])
	})
})

describe("sampler", () => {
	const log = {warn: vi.fn()}
	it("hands each sampled proposal to the shadow's detail comparison", async () => {
		queries.getProposalWithVotes.mockImplementation((id: string) => ({id}))
		const shadow = {detail: vi.fn(), list: vi.fn()}
		const s = createGovernanceSampler({
			config: {enabled: true, intervalMs: 1, sample: 2},
			db: idRows(["a", "b"]),
			shadow,
			log,
			random: () => "p",
		})
		await s.runOnce()
		expect(shadow.detail).toHaveBeenCalledTimes(2)
		expect(shadow.detail.mock.calls[0]?.[0]).toEqual({id: "a"})
	})
	it("skips a proposal that is gone and never throws", async () => {
		queries.getProposalWithVotes.mockImplementation(() => null)
		const shadow = {detail: vi.fn(), list: vi.fn()}
		const s = createGovernanceSampler({
			config: {enabled: true, intervalMs: 1, sample: 1},
			db: idRows(["a"]),
			shadow,
			log,
		})
		await s.runOnce()
		expect(shadow.detail).not.toHaveBeenCalled()
		const broken = createGovernanceSampler({
			config: {enabled: true, intervalMs: 1, sample: 1},
			db: {execute: async () => Promise.reject(new Error("down"))} as unknown as Database,
			shadow,
			log,
		})
		await expect(broken.runOnce()).resolves.toBeUndefined()
		expect(log.warn).toHaveBeenCalled()
	})
	it("start is inert when disabled", () => {
		const s = createGovernanceSampler({
			config: {enabled: false, intervalMs: 1, sample: 1},
			db: idRows([]),
			shadow: {detail: vi.fn(), list: vi.fn()},
			log,
		})
		expect(s.start()()).toBeUndefined()
	})
})
