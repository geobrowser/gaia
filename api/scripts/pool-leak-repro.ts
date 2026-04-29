#!/usr/bin/env bun
/**
 * Reproduction script for the GraphQL pool connection leak.
 * Companion to api/docs/gql-pool-leak-investigation.md.
 *
 * Fires N copies of a known-slow `EntitiesOrderedByProperty` query against the
 * configured GraphQL endpoint, aborting each one mid-flight. Each cancelled
 * request that lands during SQL execution should leak one slot from the pg
 * pool of whichever api replica handled it (pre-fix). After deploying the fix,
 * the same cycle should produce zero leaked slots.
 *
 * USAGE
 *   bun api/scripts/pool-leak-repro.ts
 *
 * TUNABLES (env vars)
 *   ENDPOINT          target /graphql URL (default: testnet)
 *   ATTEMPTS          number of aborted requests (default: 20)
 *   ABORT_AFTER_MS    abort each request after this many ms (default: 1500)
 *   PAUSE_BETWEEN_MS  delay between attempts (default: 500)
 *   START_OFFSET      starting `offset` variable; we increment per request
 *                     to force cache misses (default: 9)
 *
 * WHAT TO WATCH (in another terminal / dashboard)
 *   Sentry:    graphql.pool.total_connections - graphql.pool.idle_connections
 *   PgBouncer: SHOW POOLS;   (sv_active for the api db/user)
 *   Postgres:  SELECT count(*) FROM pg_stat_activity
 *              WHERE application_name LIKE '%postgraphile%' AND state != 'idle';
 *
 * EXPECTED OUTCOME
 *   Pre-fix:   checked_out grows by ~ATTEMPTS (modulo replica spread) and
 *              stays elevated until the next pod restart. This is the leak.
 *   Post-fix:  checked_out returns to baseline within seconds (in-flight
 *              queries finish, onResponse releases). No persistent growth.
 *
 * SAFETY NOTES
 *   - Production has PG_POOL_PRESSURE_WAITING_THRESHOLD=1, so excessive abort
 *     cycles can trip the saturation FSM and start shedding 503s for other
 *     users. Keep ATTEMPTS modest (default 20 against a max-50 pool, spread
 *     across replicas, leaves plenty of headroom).
 *   - Leaked slots persist until pod restart. If you accidentally exhaust the
 *     pool: kubectl rollout restart deployment/api -n api
 *   - This is a destructive test against shared infrastructure. Coordinate
 *     before running in production.
 */

const ENDPOINT = process.env.ENDPOINT ?? "https://testnet-api.geobrowser.io/graphql"
const ATTEMPTS = parseInt(process.env.ATTEMPTS ?? "20", 10)
const ABORT_AFTER_MS = parseInt(process.env.ABORT_AFTER_MS ?? "1500", 10)
const PAUSE_BETWEEN_MS = parseInt(process.env.PAUSE_BETWEEN_MS ?? "500", 10)
const START_OFFSET = parseInt(process.env.START_OFFSET ?? "9", 10)
const WARMUP_TIMEOUT_MS = 30_000

const TEST_RUN_ID = `pool-leak-${Date.now().toString(36)}-${crypto.randomUUID().slice(0, 8)}`

// Slow query — verbatim shape from the EntitiesOrderedByProperty operation
// flagged in production logs. Heavy nested fetch (entities + their values +
// relations + each related entity's values + types).
const QUERY = `query EntitiesOrderedByProperty($propertyId: UUID, $sortDirection: SortOrder, $dataType: String, $spaceId: UUID, $limit: Int, $offset: Int, $filter: EntityFilter) {
  entitiesOrderedByProperty(
    propertyId: $propertyId
    sortDirection: $sortDirection
    dataType: $dataType
    spaceId: $spaceId
    first: $limit
    offset: $offset
    filter: $filter
  ) {
    id
    name
    description
    spaceIds
    updatedAt
    types { id name }
    valuesList(filter: {spaceId: {is: $spaceId}}) {
      spaceId
      property { ...PropertyFragment }
      text integer float point boolean time language unit datetime date decimal bytes schedule
    }
    relationsList(filter: {spaceId: {is: $spaceId}}) {
      id spaceId position verified entityId
      fromEntity { id name }
      toEntity {
        id name
        types { id name }
        valuesList {
          spaceId propertyId text integer float point boolean time datetime date decimal bytes schedule
        }
      }
      toSpaceId
      type { id name }
    }
  }
}

fragment PropertyFragment on PropertyInfo {
  id name dataTypeId dataTypeName renderableTypeId renderableTypeName format isType
}`

function buildVariables(offset: number) {
	return {
		propertyId: "a126ca530c8e48d5b88882c734c38935",
		sortDirection: "ASC",
		dataType: "text",
		limit: 10,
		offset,
		filter: {
			typeIds: {anyEqualTo: "7ed45f2bc48b419e8e4664d5ff680b0d"},
			spaceIds: {in: ["89bd89bf28ff8a0963faf92a8c905e20"]},
		},
	}
}

function buildBody(offset: number) {
	return JSON.stringify({
		query: QUERY,
		variables: buildVariables(offset),
		operationName: "EntitiesOrderedByProperty",
	})
}

function buildHeaders(reqIdSuffix: string) {
	const reqId = `${TEST_RUN_ID}-${reqIdSuffix}`
	return {
		accept: "application/graphql-response+json, application/json",
		"content-type": "application/json",
		origin: "https://www.geobrowser.io",
		referer: "https://www.geobrowser.io/",
		"user-agent": "pool-leak-repro/1.0",
		"x-correlation-id": reqId,
		"x-request-id": reqId,
	}
}

function ts() {
	return new Date().toISOString()
}

async function sleep(ms: number) {
	return new Promise((resolve) => setTimeout(resolve, ms))
}

function isAbortError(err: unknown): boolean {
	return err instanceof Error && (err.name === "AbortError" || err.name === "TimeoutError")
}

// Warmup: confirm the endpoint is actually a GraphQL API and the query is
// slow enough that ABORT_AFTER_MS lands during execution. If either check
// fails, bail with a clear error before doing anything destructive.
async function warmup(): Promise<{durationMs: number; status: number; isGraphql: boolean}> {
	console.log(`[${ts()}] warmup: firing one full query (offset=${START_OFFSET}), no abort`)
	const start = Date.now()
	const controller = new AbortController()
	const timeout = setTimeout(() => controller.abort(), WARMUP_TIMEOUT_MS)

	try {
		const res = await fetch(ENDPOINT, {
			method: "POST",
			headers: buildHeaders("warmup"),
			body: buildBody(START_OFFSET),
			signal: controller.signal,
		})
		const durationMs = Date.now() - start
		const text = await res.text()
		console.log(`[${ts()}] warmup: status=${res.status} duration=${durationMs}ms`)
		console.log(`[${ts()}] warmup: body[0..200]=${text.slice(0, 200)}`)

		// Validate the response looks like a GraphQL response. A wrong endpoint
		// (e.g. a frontend dev server on the same port) might return HTML with
		// status 200, and the duration check alone won't catch it.
		const contentType = res.headers.get("content-type") ?? ""
		const isJsonContentType = /\b(application\/json|application\/graphql-response\+json)\b/i.test(contentType)
		let isGraphql = false
		if (isJsonContentType) {
			try {
				const parsed = JSON.parse(text)
				isGraphql = typeof parsed === "object" && parsed !== null && ("data" in parsed || "errors" in parsed)
			} catch {
				isGraphql = false
			}
		}

		return {durationMs, status: res.status, isGraphql}
	} finally {
		clearTimeout(timeout)
	}
}

async function abortRun(i: number, offset: number): Promise<{outcome: string; durationMs: number}> {
	const start = Date.now()
	const controller = new AbortController()
	const timeout = setTimeout(() => controller.abort(), ABORT_AFTER_MS)

	try {
		const res = await fetch(ENDPOINT, {
			method: "POST",
			headers: buildHeaders(`abort-${i}`),
			body: buildBody(offset),
			signal: controller.signal,
		})
		const durationMs = Date.now() - start
		// If the server returned BEFORE our abort timer fired, the abort never
		// landed during execution — this run does NOT exercise the leak path.
		return {outcome: `unexpectedly completed: status=${res.status}`, durationMs}
	} catch (err) {
		const durationMs = Date.now() - start
		if (isAbortError(err)) {
			return {outcome: "aborted (expected)", durationMs}
		}
		return {outcome: `errored: ${(err as Error).message}`, durationMs}
	} finally {
		clearTimeout(timeout)
	}
}

async function waitForEnter(prompt: string): Promise<void> {
	if (!process.stdin.isTTY) {
		console.log(`${prompt} (non-interactive — continuing immediately)`)
		return
	}
	process.stdout.write(prompt)
	await new Promise<void>((resolve) => {
		process.stdin.resume()
		const handler = () => {
			process.stdin.pause()
			process.stdin.removeListener("data", handler)
			resolve()
		}
		process.stdin.on("data", handler)
	})
}

async function main() {
	console.log(`========================================`)
	console.log(`Pool leak reproduction`)
	console.log(`========================================`)
	console.log(`endpoint:         ${ENDPOINT}`)
	console.log(`attempts:         ${ATTEMPTS}`)
	console.log(`abort_after_ms:   ${ABORT_AFTER_MS}`)
	console.log(`pause_between_ms: ${PAUSE_BETWEEN_MS}`)
	console.log(`offset range:     ${START_OFFSET}..${START_OFFSET + ATTEMPTS}`)
	console.log(`test_run_id:      ${TEST_RUN_ID}`)
	console.log(`========================================\n`)

	// 1. Warmup
	let warmupResult: {durationMs: number; status: number; isGraphql: boolean}
	try {
		warmupResult = await warmup()
	} catch (err) {
		console.error(`[${ts()}] warmup failed:`, err)
		process.exit(1)
	}

	if (!warmupResult.isGraphql) {
		console.error(
			`[${ts()}] FATAL: endpoint did not return a GraphQL response (no JSON with data/errors).`,
		)
		console.error(`Check ENDPOINT — it may be pointing at a frontend dev server, a docs site,`)
		console.error(`or a different service. The repro requires a real /graphql endpoint.`)
		console.error(`To target a single api pod via port-forward:`)
		console.error(`  kubectl -n api port-forward pod/<api-pod-name> 8080:3000`)
		console.error(`  ENDPOINT=http://localhost:8080/graphql bun api/scripts/pool-leak-repro.ts`)
		process.exit(1)
	}

	if (warmupResult.status !== 200) {
		console.error(
			`[${ts()}] warmup returned non-200 status (${warmupResult.status}). Check the query / endpoint and retry.`,
		)
		process.exit(1)
	}

	const minSafeQueryMs = ABORT_AFTER_MS + 500
	if (warmupResult.durationMs < minSafeQueryMs) {
		console.error(
			`[${ts()}] FATAL: query completed in ${warmupResult.durationMs}ms — too fast for ABORT_AFTER_MS=${ABORT_AFTER_MS}.`,
		)
		console.error(`Aborts will fire AFTER the response returns, so they won't exercise the leak path.`)
		console.error(`Reduce ABORT_AFTER_MS or pick a slower query.`)
		process.exit(1)
	}

	console.log()
	console.log(`========================================`)
	console.log(`Capture BASELINE pool stats now.`)
	console.log(`========================================`)
	console.log(`In another terminal/dashboard, take note of:`)
	console.log(`  - Sentry: graphql.pool.total_connections, graphql.pool.idle_connections`)
	console.log(`  - PgBouncer: SHOW POOLS;  (sv_active for the api user)`)
	console.log(`  - Postgres pg_stat_activity (if accessible)`)
	console.log()
	await waitForEnter("Press ENTER when baseline captured to start abort cycle... ")
	console.log()

	// 2. Abort cycle
	const cycleStart = ts()
	console.log(`[${cycleStart}] ===== abort cycle started =====`)
	const stats = {aborted: 0, unexpectedCompletion: 0, errored: 0}
	for (let i = 1; i <= ATTEMPTS; i++) {
		const offset = START_OFFSET + i // unique per request to force cache miss
		const {outcome, durationMs} = await abortRun(i, offset)
		const tag = outcome.startsWith("aborted")
			? "abort"
			: outcome.startsWith("unexpectedly")
				? "compl"
				: "error"
		console.log(`[${ts()}] ${String(i).padStart(2)}/${ATTEMPTS} [${tag}] ${outcome} after ${durationMs}ms`)
		if (outcome.startsWith("aborted")) stats.aborted++
		else if (outcome.startsWith("unexpectedly")) stats.unexpectedCompletion++
		else stats.errored++

		if (i < ATTEMPTS) await sleep(PAUSE_BETWEEN_MS)
	}
	const cycleEnd = ts()
	console.log(`[${cycleEnd}] ===== abort cycle finished =====\n`)

	// 3. Summary + post-test instructions
	console.log(`========================================`)
	console.log(`Summary`)
	console.log(`========================================`)
	console.log(`aborted (expected, exercised leak path):  ${stats.aborted}`)
	console.log(`unexpected completion (no leak window):    ${stats.unexpectedCompletion}`)
	console.log(`errored (network/server issue):            ${stats.errored}`)
	console.log()
	console.log(`Time window for log search: ${cycleStart} → ${cycleEnd}`)
	console.log(`Filter server logs by: x-request-id starts with "${TEST_RUN_ID}-abort-"`)
	console.log()
	console.log(`========================================`)
	console.log(`Wait ~30s for in-flight queries to settle`)
	console.log(`(Postgres statement_timeout = 10s; PgBouncer query_timeout = 15s),`)
	console.log(`then capture POST-TEST pool stats.`)
	console.log(`========================================`)
	console.log()
	console.log(`Expected reading:`)
	console.log(`  PRE-FIX (current main):  checked_out increased by ~${stats.aborted}`)
	console.log(`                           and STAYS elevated → leak confirmed`)
	console.log(`  POST-FIX (this branch):  checked_out returns to baseline within seconds`)
	console.log(`                           → fix validated`)
	console.log()
	console.log(`Reminder: with multiple replicas, the increase is spread across pods.`)
	console.log(`To target a single pod: kubectl port-forward to one api pod and set`)
	console.log(`ENDPOINT to that local port.`)
}

main().catch((err) => {
	console.error("script failed:", err)
	process.exit(1)
})
