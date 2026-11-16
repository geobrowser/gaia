/**
 * Process and JavaScriptCore heap gauges for /metrics, to find what grows in an api pod (#1600).
 *
 * An api pod's working set climbs ~25-50 MiB an hour from boot until it is OOMKilled at 2Gi, and the growth is anonymous
 * memory with a flat file-descriptor count. Bun can't be attached to from outside the pod, so the split between the JS heap
 * and native memory, and which JS object types grow, has to come from inside the process. These gauges give that:
 *
 * - `gaia_api_process_memory_bytes{kind}`: process.memoryUsage() — rss, heap_total, heap_used, external, array_buffers.
 *   rss growing while heap_used stays flat points at native memory (Bun/JSC internals, buffers) rather than JS objects.
 * - `gaia_api_jsc_heap_bytes{kind}`: bun:jsc heapStats() size, capacity and extra memory.
 * - `gaia_api_jsc_heap_objects{kind}`: total and protected object counts.
 * - `gaia_api_jsc_objects{type}`: live object count for the MAX_TYPES most numerous object types, which names whatever leaks.
 *
 * heapStats() takes a census of the heap, so it is computed at most once per CACHE_MS however often /metrics is scraped.
 * Outside Bun (vitest under node) the JSC gauges are left out.
 */

const MAX_TYPES = 30
const CACHE_MS = 60_000

type HeapStats = {
	heapSize: number
	heapCapacity: number
	extraMemorySize: number
	objectCount: number
	protectedObjectCount: number
	objectTypeCounts: Record<string, number>
}

type Snapshot = {at: number; stats: HeapStats | null}

let cached: Snapshot | null = null

function loadHeapStats(): (() => HeapStats) | null {
	if (typeof (globalThis as {Bun?: unknown}).Bun === "undefined") return null
	try {
		// require keeps bun:jsc out of module resolution under node, where it doesn't exist.
		const jsc = require("bun:jsc") as {heapStats: () => HeapStats}
		return jsc.heapStats
	} catch {
		return null
	}
}

const heapStatsFn = loadHeapStats()

function escapeLabel(value: string): string {
	return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n")
}

function family(name: string, help: string, label: string, rows: Array<[string, number]>): string {
	const lines = [`# HELP ${name} ${help}`, `# TYPE ${name} gauge`]
	for (const [key, value] of rows) lines.push(`${name}{${label}="${escapeLabel(key)}"} ${value}`)
	return lines.join("\n")
}

/** Keep the MAX_TYPES most numerous types, largest first. */
export function topObjectTypes(counts: Record<string, number>, max = MAX_TYPES): Array<[string, number]> {
	return Object.entries(counts)
		.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
		.slice(0, max)
}

function heapSnapshot(now: number): HeapStats | null {
	if (!heapStatsFn) return null
	if (cached && now - cached.at < CACHE_MS) return cached.stats
	let stats: HeapStats | null = null
	try {
		stats = heapStatsFn()
	} catch {
		stats = null
	}
	cached = {at: now, stats}
	return stats
}

export function renderRuntimeMemoryMetrics(now = Date.now()): string {
	const mem = process.memoryUsage()
	const parts = [
		family("gaia_api_process_memory_bytes", "process.memoryUsage() of the api process, by kind.", "kind", [
			["rss", mem.rss],
			["heap_total", mem.heapTotal],
			["heap_used", mem.heapUsed],
			["external", mem.external],
			["array_buffers", mem.arrayBuffers ?? 0],
		]),
	]
	const stats = heapSnapshot(now)
	if (stats) {
		parts.push(
			family(
				"gaia_api_jsc_heap_bytes",
				"JavaScriptCore heap from bun:jsc heapStats(), refreshed at most once a minute.",
				"kind",
				[
					["size", stats.heapSize],
					["capacity", stats.heapCapacity],
					["extra", stats.extraMemorySize],
				],
			),
			family("gaia_api_jsc_heap_objects", "JavaScriptCore live object counts from bun:jsc heapStats().", "kind", [
				["total", stats.objectCount],
				["protected", stats.protectedObjectCount],
			]),
			family(
				"gaia_api_jsc_objects",
				`Live objects of each of the ${MAX_TYPES} most numerous JavaScriptCore object types (bun:jsc heapStats().objectTypeCounts).`,
				"type",
				topObjectTypes(stats.objectTypeCounts ?? {}),
			),
		)
	}
	return parts.join("\n")
}

/** Test seam: forget the cached heap census. */
export function resetRuntimeMemoryMetricsCache(): void {
	cached = null
}
