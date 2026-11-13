import {describe, expect, it} from "vitest"
import type {KeyedRecordDeps} from "../keyedRecord.js"
import {describeMessage, parseRange, replayKeyedMessages} from "../replayKeyed.js"

const deps = {} as KeyedRecordDeps
const m = (offset: number, type: string) => ({offset: String(offset), value: Buffer.from("x"), headers: {"event-type": Buffer.from(type)}})

describe("parseRange", () => {
	it("reads N and N-M inclusively", () => {
		expect(parseRange("98657")).toEqual({from: 98657, to: 98657})
		expect(parseRange("172829-172833")).toEqual({from: 172829, to: 172833})
	})
	it("refuses junk, a backwards range and a wide one", () => {
		expect(() => parseRange("a-b")).toThrow()
		expect(() => parseRange("5-4")).toThrow()
		expect(() => parseRange("0-1000")).toThrow(/handful/)
	})
})

describe("replayKeyedMessages", () => {
	it("feeds each message, in offset order, to each handler and reports every outcome", async () => {
		const seen: string[] = []
		const handler = (name: string, own: string) => ({
			name,
			deps,
			handle: async (_d: KeyedRecordDeps, _p: number, msg: {offset: string; headers?: Record<string, unknown>}) => {
				seen.push(`${name}@${msg.offset}`)
				return Buffer.from(msg.headers?.["event-type"] as Buffer).toString() === own ? ("applied" as const) : ("ignored" as const)
			},
		})
		const r = await replayKeyedMessages(0, [m(2, "B"), m(1, "A")], [handler("a", "A"), handler("b", "B")])
		expect(seen).toEqual(["a@1", "b@1", "a@2", "b@2"])
		expect(r).toEqual([
			{offset: "1", handler: "a", outcome: "applied"},
			{offset: "1", handler: "b", outcome: "ignored"},
			{offset: "2", handler: "a", outcome: "ignored"},
			{offset: "2", handler: "b", outcome: "applied"},
		])
	})
	it("lets a handler failure stop the replay", async () => {
		const boom = {name: "x", deps, handle: async () => Promise.reject(new Error("serve down"))}
		await expect(replayKeyedMessages(0, [m(1, "A")], [boom])).rejects.toThrow("serve down")
	})
})

describe("describeMessage", () => {
	it("names the event type and size", () => {
		expect(describeMessage(m(7, "PROPOSAL_EXECUTED"))).toEqual({offset: "7", eventType: "PROPOSAL_EXECUTED", bytes: 1})
	})
})
