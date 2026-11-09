import {afterEach, describe, expect, it, vi} from "vitest"
import {createIntervalSampler, DEFAULT_SAMPLER_INTERVAL_MS, readIntervalSamplerConfig} from "../intervalSampler"

afterEach(() => vi.useRealTimers())

describe("readIntervalSamplerConfig", () => {
	it("is off by default, with a five-minute interval", () => {
		expect(readIntervalSamplerConfig({}, "FLAG", "MS")).toEqual({enabled: false, intervalMs: 300_000})
		expect(DEFAULT_SAMPLER_INTERVAL_MS).toBe(300_000)
	})
	it("reads the flag as 1, on or true, and takes a whole-number interval capped at a day", () => {
		for (const v of ["1", "on", "TRUE", " true "])
			expect(readIntervalSamplerConfig({FLAG: v}, "FLAG", "MS").enabled).toBe(true)
		expect(readIntervalSamplerConfig({FLAG: "0"}, "FLAG", "MS").enabled).toBe(false)
		expect(readIntervalSamplerConfig({MS: "2500"}, "FLAG", "MS").intervalMs).toBe(2500)
		expect(readIntervalSamplerConfig({MS: "999999999999"}, "FLAG", "MS").intervalMs).toBe(86_400_000)
		for (const bad of ["0", "-5", "1.5", "x", ""])
			expect(readIntervalSamplerConfig({MS: bad}, "FLAG", "MS").intervalMs).toBe(300_000)
		expect(readIntervalSamplerConfig({}, "FLAG", "MS", 30_000).intervalMs).toBe(30_000)
	})
})

describe("createIntervalSampler", () => {
	const make = (over: {enabled?: boolean; pass?: () => Promise<void>; onBusy?: () => void} = {}) => {
		const warn = vi.fn()
		const pass = vi.fn(over.pass ?? (async () => {}))
		const sampler = createIntervalSampler({
			enabled: over.enabled ?? true,
			intervalMs: 1000,
			pass,
			onBusy: over.onBusy,
			log: {warn},
			label: "test shadow",
		})
		return {sampler, pass, warn}
	}

	it("starts no timer when off", () => {
		const spy = vi.spyOn(globalThis, "setInterval")
		const {sampler, pass} = make({enabled: false})
		sampler.start()()
		expect(spy).not.toHaveBeenCalled()
		expect(pass).not.toHaveBeenCalled()
		spy.mockRestore()
	})
	it("runs every interval, unrefs its timer so it cannot hold the process open, and stops", async () => {
		vi.useFakeTimers()
		const unref = vi.fn()
		const real = globalThis.setInterval
		const spy = vi.spyOn(globalThis, "setInterval").mockImplementation(((fn: () => void, ms: number) => {
			const t = real(fn, ms)
			const wrapped = t as unknown as {unref: () => unknown}
			const original = wrapped.unref.bind(t)
			wrapped.unref = () => {
				unref()
				return original()
			}
			return t
		}) as typeof setInterval)
		try {
			const {sampler, pass} = make()
			const stop = sampler.start()
			expect(unref).toHaveBeenCalledTimes(1)
			await vi.advanceTimersByTimeAsync(3000)
			expect(pass).toHaveBeenCalledTimes(3)
			stop()
			await vi.advanceTimersByTimeAsync(5000)
			expect(pass).toHaveBeenCalledTimes(3)
			expect(vi.getTimerCount()).toBe(0)
		} finally {
			spy.mockRestore()
		}
	})
	it("skips a tick while a pass is running and reports it, then runs again", async () => {
		let release: () => void = () => {}
		const onBusy = vi.fn()
		const {sampler, pass} = make({
			onBusy,
			pass: () =>
				new Promise<void>((r) => {
					release = r
				}),
		})
		const first = sampler.runOnce()
		await sampler.runOnce()
		expect(pass).toHaveBeenCalledTimes(1)
		expect(onBusy).toHaveBeenCalledTimes(1)
		release()
		await first
		const second = sampler.runOnce()
		expect(pass).toHaveBeenCalledTimes(2)
		release()
		await second
	})
	it("never throws: a rejecting pass is logged and the next one still runs", async () => {
		const {sampler, pass, warn} = make({
			pass: async () => {
				throw new Error("boom")
			},
		})
		await expect(sampler.runOnce()).resolves.toBeUndefined()
		await sampler.runOnce()
		expect(pass).toHaveBeenCalledTimes(2)
		expect(warn).toHaveBeenCalledTimes(2)
	})
})
