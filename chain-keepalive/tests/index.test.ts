/**
 * Tests for the pure decision logic in src/index.ts — the RPC/chain-touching
 * parts (getBlock, sendTransaction, waitForTransactionReceipt) are exercised
 * manually against the live chain instead, since faking a chain client
 * wouldn't verify anything except that the fake behaves as programmed.
 */
import {describe, expect, test} from "bun:test"
import {parseGwei} from "viem"
import {idleMsSince, shouldSendKeepAlive, withFeeFloor} from "../src/index.js"

describe("idleMsSince", () => {
	test("computes elapsed time from a block timestamp in seconds to now in ms", () => {
		const now = 1_000_000_000_000
		const blockTimestampSeconds = 999_999_940n // 60s before `now`
		expect(idleMsSince(blockTimestampSeconds, now)).toBe(60_000)
	})

	test("returns 0 for a block timestamped exactly now", () => {
		const now = 1_000_000_000_000
		expect(idleMsSince(BigInt(now / 1000), now)).toBe(0)
	})
})

describe("shouldSendKeepAlive", () => {
	test("false when idle time is under the threshold", () => {
		expect(shouldSendKeepAlive(5 * 60_000, 10 * 60_000)).toBe(false)
	})

	test("true when idle time meets the threshold exactly", () => {
		expect(shouldSendKeepAlive(10 * 60_000, 10 * 60_000)).toBe(true)
	})

	test("true when idle time exceeds the threshold", () => {
		expect(shouldSendKeepAlive(20 * 60_000, 10 * 60_000)).toBe(true)
	})
})

describe("withFeeFloor", () => {
	const floor = parseGwei("1")

	test("doubles the estimate when that's already above the floor", () => {
		const estimate = parseGwei("2")
		expect(withFeeFloor(estimate, floor)).toBe(parseGwei("4"))
	})

	test("uses the floor when double the estimate falls short", () => {
		const estimate = parseGwei("0.01")
		expect(withFeeFloor(estimate, floor)).toBe(floor)
	})

	test("uses the floor for a zero estimate", () => {
		expect(withFeeFloor(0n, floor)).toBe(floor)
	})

	test("floor boundary: exactly double the floor stays the doubled estimate, not the floor", () => {
		const estimate = floor // doubled = 2x floor, which is > floor
		expect(withFeeFloor(estimate, floor)).toBe(floor * 2n)
	})
})
