/**
 * The geo-db twin of proposal detection, behind PROPOSAL_EXECUTOR_GEODB_TWIN (off by default).
 *
 * Each tick, after SQL detection has produced the set to execute, the twin takes a candidate set from Postgres,
 * decides readiness for each candidate from geo-db records (evaluate.ts), and compares with the SQL set. Counts and a
 * `geodb_twin_summary` log line leave the run; a `geodb_twin_mismatch` line names each disagreement. SQL stays the
 * source: nothing here returns anything execution reads, every failure is swallowed, and the whole twin has a time
 * budget past which it is abandoned.
 *
 * Candidates. geo-db has no read that lists proposals, so the candidates come from Postgres: Slow proposals inside the
 * execution window (created less than MAX_PROPOSAL_AGE ago, or `execute_by` not yet passed), executed or not, capped.
 * The SQL ready set is always included. Including executed proposals is deliberate: they are how the twin notices
 * geo-db missing an `executed_at`. The comparison covers only the candidates, so a proposal outside them is not judged.
 *
 * The candidate query runs on its own short-lived connection with a statement timeout, never on the execution client,
 * so a slow query cannot hold up execution.
 */
import {readFile} from "node:fs/promises"
import {dirname, join} from "node:path"
import {Effect} from "effect"
import Pg from "pg"

import {MAX_PROPOSAL_AGE, type Proposal} from "../detect.js"
import {createGeoDbTwinClient, type GeoDbTwinReader} from "./client.js"
import {evaluateProposal, type Spaces, type UnsupportedReason} from "./evaluate.js"
import {
	type MismatchKind,
	recordCompared,
	recordMismatch,
	recordRun,
	recordUnsupported,
	snapshot,
	type TwinResult,
} from "./metrics.js"

export interface TwinConfig {
	clientBin: string
	keyPath: string
	bindingPath: string
	spaceMapPath: string
	/** Per geo-db read, ms. */
	timeoutMs: number
	/** Whole twin, ms. */
	budgetMs: number
	/** Most candidates taken (the SQL ready set is added to them). */
	candidateMax: number
}

const DEFAULTS = {timeoutMs: 2_000, budgetMs: 20_000, candidateMax: 100}

const posInt = (raw: string | undefined, fallback: number, max: number): number => {
	const n = Number(raw)
	return Number.isInteger(n) && n > 0 ? Math.min(n, max) : fallback
}

/**
 * The twin's config from the environment, or null when the flag is off or a required path is missing. A missing path
 * with the flag on is logged by the caller as `skipped_misconfigured`: the twin never starts half-configured.
 */
export function loadTwinConfig(env: Record<string, string | undefined>): TwinConfig | "misconfigured" | null {
	if (env.PROPOSAL_EXECUTOR_GEODB_TWIN !== "true") return null
	const clientBin = env.GEODB_CLIENT_BIN ?? ""
	const keyPath = env.GEODB_KEY_PATH ?? ""
	const bindingPath = env.GEODB_BINDING_PATH ?? ""
	const spaceMapPath = env.GEODB_SPACE_MAP_PATH ?? ""
	if (!clientBin || !keyPath || !bindingPath || !spaceMapPath) return "misconfigured"
	return {
		clientBin,
		keyPath,
		bindingPath,
		spaceMapPath,
		timeoutMs: posInt(env.PROPOSAL_EXECUTOR_GEODB_TWIN_TIMEOUT_MS, DEFAULTS.timeoutMs, 30_000),
		budgetMs: posInt(env.PROPOSAL_EXECUTOR_GEODB_TWIN_BUDGET_MS, DEFAULTS.budgetMs, 120_000),
		candidateMax: posInt(env.PROPOSAL_EXECUTOR_GEODB_TWIN_CANDIDATE_MAX, DEFAULTS.candidateMax, 1_000),
	}
}

const HEX32 = /^[0-9a-f]{32}$/
const compact = (id: string) => id.replaceAll("-", "").toLowerCase()

/** gaia space id (compact) -> geo-db space id; a `loaded: false` entry is left out, as the api's map does. */
export function parseSpaceMap(text: string): Map<string, string> {
	const raw: unknown = JSON.parse(text)
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("space map is not an object")
	const map = new Map<string, string>()
	for (const [key, entry] of Object.entries(raw)) {
		const gaia = compact(key)
		const geo = compact(String((entry as {geo?: unknown} | null)?.geo ?? ""))
		if (!HEX32.test(gaia) || !HEX32.test(geo)) throw new Error("space map entry is not a pair of space ids")
		if ((entry as {loaded?: unknown} | null)?.loaded === false) continue
		map.set(gaia, geo)
	}
	return map
}

/** family -> geo-db space id, from `system-spaces.json`. */
export function parseSystemSpaces(text: string): Map<string, string> {
	const raw: unknown = JSON.parse(text)
	if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("system spaces is not an object")
	const map = new Map<string, string>()
	for (const [family, entry] of Object.entries(raw)) {
		const geo = compact(String((entry as {geo?: unknown} | null)?.geo ?? ""))
		if (HEX32.test(geo)) map.set(family, geo)
	}
	return map
}

/** The Space lookups, read once (the run is one tick). `system-spaces.json` sits beside the space map. */
export async function loadSpaces(spaceMapPath: string): Promise<Spaces> {
	const spaceMap = parseSpaceMap(await readFile(spaceMapPath, "utf8"))
	const system = parseSystemSpaces(await readFile(join(dirname(spaceMapPath), "system-spaces.json"), "utf8"))
	return {
		governance: system.get("governance") ?? null,
		votes: system.get("votes") ?? null,
		of: (id) => spaceMap.get(compact(id)) ?? null,
	}
}

/**
 * Slow proposals in the execution window, oldest first, at most `limit`. Executed and unexecutable ones are kept so the
 * twin's reading of those columns is compared too. Deliberately the same window arithmetic as DETECTION_SQL.
 */
export const CANDIDATE_SQL = `
SELECT pc.id, pc.space_id AS "spaceId"
FROM proposals_current pc
WHERE pc.voting_mode = 'Slow'
  AND pc.created_at::bigint <= $1::bigint
  AND (
    (pc.execute_by IS NOT NULL AND $1::bigint <= pc.execute_by)
    OR (pc.execute_by IS NULL AND $1::bigint - pc.created_at::bigint < ${MAX_PROPOSAL_AGE})
  )
ORDER BY pc.created_at::bigint DESC
LIMIT $2
`

export type TwinDeps = {
	/** Candidate proposals (dashed ids) for this tick. */
	candidates(nowSeconds: number, limit: number): Promise<Proposal[]>
	reader: GeoDbTwinReader
	spaces: Spaces
	/** True once the reader has failed; the twin stops there. */
	failed(): boolean
	now(): number
}

export type TwinOutcome = {
	compared: number
	match: number
	mismatch: number
	unsupported: number
	failed: number
	aborted: "failure" | "budget" | null
}

export type MismatchLog = {proposalId: string; kind: MismatchKind; sql: boolean; geodb: boolean; why: string}

/**
 * Compare the geo-db verdict with the SQL set. `sqlReady` is the set execution will use, ids as detection returned
 * them. Never throws: a failed read counts `failed` and ends the run (the cool-down).
 */
export async function compareWithGeoDb(
	deps: TwinDeps,
	sqlReady: readonly Proposal[],
	nowSeconds: number,
	config: Pick<TwinConfig, "budgetMs" | "candidateMax">,
	onMismatch: (m: MismatchLog) => void = () => {},
): Promise<TwinOutcome> {
	const out: TwinOutcome = {compared: 0, match: 0, mismatch: 0, unsupported: 0, failed: 0, aborted: null}
	const deadline = deps.now() + config.budgetMs
	const sqlIds = new Set(sqlReady.map((p) => compact(p.id)))
	const byId = new Map<string, Proposal>()
	for (const p of sqlReady) byId.set(compact(p.id), p)
	for (const p of await deps.candidates(nowSeconds, config.candidateMax))
		if (!byId.has(compact(p.id))) byId.set(compact(p.id), p)

	const note = (result: TwinResult) => {
		out.compared++
		out[result]++
		recordCompared(result)
	}

	for (const [key, proposal] of byId) {
		if (deps.failed()) {
			out.aborted = "failure"
			break
		}
		if (deps.now() >= deadline) {
			out.aborted = "budget"
			break
		}
		let verdict: Awaited<ReturnType<typeof evaluateProposal>>
		try {
			verdict = await evaluateProposal(deps.reader, deps.spaces, proposal, nowSeconds)
		} catch {
			note("failed")
			out.aborted = "failure"
			break
		}
		if (verdict.kind === "unsupported") {
			note("unsupported")
			recordUnsupported(verdict.reason satisfies UnsupportedReason)
			continue
		}
		const sql = sqlIds.has(key)
		if (verdict.ready === sql) {
			note("match")
			continue
		}
		note("mismatch")
		const kind: MismatchKind = verdict.ready ? "extra_in_geodb" : "missing_in_geodb"
		recordMismatch(kind)
		onMismatch({proposalId: proposal.id, kind, sql, geodb: verdict.ready, why: verdict.why})
	}
	return out
}

/** Candidates on a connection of their own, with a statement timeout; closed before the geo-db reads begin. */
async function queryCandidates(databaseUrl: string, nowSeconds: number, limit: number): Promise<Proposal[]> {
	const client = new Pg.Client({
		connectionString: databaseUrl,
		connectionTimeoutMillis: 5_000,
		statement_timeout: 5_000,
		query_timeout: 5_000,
		application_name: "proposal-executor-geodb-twin",
	})
	await client.connect()
	try {
		const res = await client.query<Proposal>(CANDIDATE_SQL, [nowSeconds, limit])
		return res.rows
	} finally {
		await client.end().catch(() => {})
	}
}

/**
 * The twin as an Effect that cannot fail and cannot take longer than `budgetMs` plus a short grace. It is run after
 * detection and before execution, so SQL and the twin judge the same `nowSeconds`; it returns nothing.
 */
export const runGeoDbTwin = (
	env: Record<string, string | undefined>,
	databaseUrl: string,
	sqlReady: readonly Proposal[],
	nowSeconds: number,
): Effect.Effect<void> => {
	const config = loadTwinConfig(env)
	if (config === null) return Effect.void
	if (config === "misconfigured")
		return Effect.logWarning("geodb_twin_skipped").pipe(Effect.annotateLogs({reason: "misconfigured"}))

	const work = Effect.acquireUseRelease(
		Effect.sync(() =>
			createGeoDbTwinClient({
				clientBin: config.clientBin,
				keyPath: config.keyPath,
				bindingPath: config.bindingPath,
				timeoutMs: config.timeoutMs,
				cooldownMs: 30_000,
			}),
		),
		(client) =>
			Effect.tryPromise(async () => {
				const spaces = await loadSpaces(config.spaceMapPath)
				const deps: TwinDeps = {
					candidates: (now, limit) => queryCandidates(databaseUrl, now, limit),
					reader: client,
					spaces,
					failed: () => client.coolingDown(),
					now: Date.now,
				}
				const mismatches: MismatchLog[] = []
				const outcome = await compareWithGeoDb(deps, sqlReady, nowSeconds, config, (m) => mismatches.push(m))
				return {outcome, mismatches}
			}),
		(client) => Effect.sync(() => client.close()),
	)

	return work.pipe(
		Effect.timeout(config.budgetMs + 2_000),
		Effect.flatMap(({outcome, mismatches}) =>
			Effect.gen(function* () {
				recordRun(outcome.aborted ? `aborted_${outcome.aborted}` : "completed")
				for (const m of mismatches)
					yield* Effect.logWarning("geodb_twin_mismatch").pipe(Effect.annotateLogs({...m}))
				yield* Effect.logInfo("geodb_twin_summary").pipe(
					Effect.annotateLogs({...outcome, sqlReady: sqlReady.length, totals: JSON.stringify(snapshot())}),
				)
			}),
		),
		// Any failure (candidate query, unreadable maps, timeout, a defect) is logged and dropped.
		Effect.catchAllCause((cause) =>
			Effect.sync(() => recordRun("error")).pipe(
				Effect.zipRight(
					Effect.logWarning("geodb_twin_error").pipe(
						Effect.annotateLogs({
							error: String(cause)
								.slice(0, 300)
								.replace(/postgresql?:\/\/[^\s]+/gi, "<redacted>"),
						}),
					),
				),
			),
		),
	)
}
