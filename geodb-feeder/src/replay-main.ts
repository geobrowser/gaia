/**
 * geodb-card-replay entrypoint (same image as the feeder). See replay.ts. Env:
 * - REPLAY_API_URL (http://api:3000; only Service `api` is accepted), REPLAY_RATE (0.5 rps, at most 1),
 * - REPLAY_REFRESH_MS (15 min), GEODB_SPACE_MAP_PATH (/etc/geodb/space-map/space-map.json),
 * - DATABASE_URL, DATABASE_SSL_CA (gaia Postgres, read only), METRICS_PORT (9090).
 */
import {readFileSync} from "node:fs"
import {errorFields, log} from "./log.js"
import {Series} from "./metrics.js"
import {ReadOnlyPg, pgConfig} from "./pgSource.js"
import {type Pools, checkTarget, loadPools, mappedSpaces, parseRate, pick, replayOne} from "./replay.js"
import {startServer} from "./server.js"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

async function main(): Promise<void> {
	const env = process.env
	const target = checkTarget(env.REPLAY_API_URL ?? "http://api:3000")
	const rate = parseRate(env.REPLAY_RATE)
	const refreshMs = Number(env.REPLAY_REFRESH_MS ?? 15 * 60_000)
	const mapPath = env.GEODB_SPACE_MAP_PATH ?? "/etc/geodb/space-map/space-map.json"
	const databaseUrl = env.DATABASE_URL
	if (!databaseUrl) throw new Error("DATABASE_URL is required")

	const requests = new Series("geodb_card_replay_requests_total", "Replayed entity-card requests by result.", "counter")
	const pairs = new Series("geodb_card_replay_pairs", "Pairs in each replay pool.", "gauge")
	const refreshed = new Series("geodb_card_replay_last_refresh_seconds", "Unix time of the last pool refresh.", "gauge")
	const rateGauge = new Series("geodb_card_replay_rate", "Configured requests per second.", "gauge")
	rateGauge.set({}, rate)
	const all = [requests, pairs, refreshed, rateGauge]
	let pools: Pools = {top: [], recent: [], random: []}
	const server = startServer(Number(env.METRICS_PORT ?? 9090), {render: () => `${all.map((s) => s.render()).join("\n")}\n`}, {
		ready: () => true,
	})

	const pg = new ReadOnlyPg(pgConfig(databaseUrl, env.DATABASE_SSL_CA))
	let mapped = new Set<string>()
	const readMap = () => {
		try {
			mapped = mappedSpaces(readFileSync(mapPath, "utf8"))
		} catch (e) {
			log("warn", "space map unreadable; keeping the last one", errorFields(e))
		}
	}
	const refresh = async () => {
		readMap()
		try {
			pools = await loadPools(pg, [...mapped])
			for (const [k, v] of Object.entries(pools)) pairs.set({pool: k}, v.length)
			refreshed.set({}, Date.now() / 1000)
			log("info", "replay pools refreshed", {spaces: mapped.size, top: pools.top.length, recent: pools.recent.length, random: pools.random.length})
		} catch (e) {
			log("warn", "replay pool refresh failed; keeping the last pools", errorFields(e))
		}
	}

	let stopping = false
	const stop = () => {
		stopping = true
	}
	process.on("SIGTERM", stop)
	process.on("SIGINT", stop)

	log("info", "replay starting", {target: target.origin, rate, refreshMs})
	await refresh()
	let nextRefresh = Date.now() + refreshMs
	let nextMapRead = Date.now() + 60_000
	const interval = 1000 / rate
	let due = Date.now()
	while (!stopping) {
		if (Date.now() >= nextRefresh) {
			nextRefresh = Date.now() + refreshMs
			await refresh()
		} else if (Date.now() >= nextMapRead) {
			// A Space mapped out between refreshes is skipped at once, not up to 15 minutes later.
			nextMapRead = Date.now() + 60_000
			readMap()
		}
		const p = pick(pools)
		if (p) await replayOne(target, p, mapped, {requests}, fetch)
		// Fixed schedule, never catching up in a burst after a slow request.
		due = Math.max(due + interval, Date.now())
		await sleep(Math.max(0, due - Date.now()))
	}
	await pg.close()
	server.close()
}

main().then(
	() => process.exit(0),
	(e) => {
		log("error", "replay failed", errorFields(e))
		process.exit(1)
	},
)
