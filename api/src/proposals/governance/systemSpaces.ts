/**
 * The geo-db ids of the feeder's system Spaces, from `system-spaces.json` (`{"<family>": {"geo": "<32 hex>", "nonce": ..}}`).
 * The feeder publishes it to the `geodb-space-map` ConfigMap under that key, beside `space-map.json`, so the api finds it
 * in the directory of GEODB_SPACE_MAP_PATH. Re-checked (one `stat`) at most every `refreshMs`; a failed reload keeps the
 * map already held. A family not founded, or a file not there yet, answers null.
 */
import {readFile, stat} from "node:fs/promises"
import {dirname, join} from "node:path"

/** Looks up any family the feeder founded; assignable to the narrower lookups the shadows take. */
export type AnySystemSpaceLookup = (family: "governance" | "votes" | "config") => Promise<string | null>

const HEX32 = /^[0-9a-f]{32}$/
const REFRESH_MS = 10_000

export function parseSystemSpaces(text: string): Map<string, string> {
	const raw: unknown = JSON.parse(text)
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("system spaces is not an object")
	const map = new Map<string, string>()
	for (const [family, entry] of Object.entries(raw)) {
		const geo = String((entry as {geo?: unknown} | null)?.geo ?? "")
			.replaceAll("-", "")
			.toLowerCase()
		if (HEX32.test(geo)) map.set(family, geo)
	}
	return map
}

export function createSystemSpaceLookup(
	spaceMapPath: string,
	options: {refreshMs?: number; now?: () => number} = {},
): AnySystemSpaceLookup {
	const path = spaceMapPath ? join(dirname(spaceMapPath), "system-spaces.json") : ""
	const refreshMs = options.refreshMs ?? REFRESH_MS
	const now = options.now ?? Date.now
	let held: {map: Map<string, string>; stamp: string} | null = null
	let checkedAt = Number.NEGATIVE_INFINITY
	let inflight: Promise<void> | null = null

	const refresh = async () => {
		const info = await stat(path)
		const stamp = `${info.mtimeMs}:${info.size}`
		if (held?.stamp === stamp) return
		held = {map: parseSystemSpaces(await readFile(path, "utf8")), stamp}
	}

	return async (family) => {
		if (!path) return null
		if (now() - checkedAt >= refreshMs) {
			checkedAt = now()
			inflight ??= refresh()
				.catch(() => {})
				.finally(() => {
					inflight = null
				})
		}
		if (inflight) await inflight
		return held?.map.get(family) ?? null
	}
}
