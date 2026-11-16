import {describe, expect, it} from "vitest"
import {renderRuntimeMemoryMetrics, topObjectTypes} from "./runtimeMemoryMetrics"

describe("runtime memory metrics", () => {
	it("renders process memory by kind", () => {
		const text = renderRuntimeMemoryMetrics()
		expect(text).toContain("# TYPE gaia_api_process_memory_bytes gauge")
		for (const kind of ["rss", "heap_total", "heap_used", "external", "array_buffers"]) {
			expect(text).toMatch(new RegExp(`gaia_api_process_memory_bytes\\{kind="${kind}"\\} \\d+`))
		}
	})

	it("leaves out the JSC gauges outside Bun", () => {
		expect(renderRuntimeMemoryMetrics()).not.toContain("gaia_api_jsc_")
	})

	it("keeps the most numerous object types, largest first, ties by name", () => {
		expect(topObjectTypes({Object: 5, Array: 9, Map: 5, Set: 1}, 3)).toEqual([
			["Array", 9],
			["Map", 5],
			["Object", 5],
		])
	})
})
