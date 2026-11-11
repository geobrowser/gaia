/**
 * Per-transaction statement_timeout for a pg Pool that sits behind PgBouncer in transaction mode.
 *
 * Why not `SET statement_timeout` on connect: PgBouncer in transaction mode hands a client's
 * statements to whichever server connection is free at that moment, and a session-level SET stays
 * on that server connection after the client moves on. Every gaia service shares one PgBouncer
 * pool, so a session SET from one pool leaks onto another pool's statements — and the pool that
 * issued it only gets its budget when it happens to land back on a connection that already
 * carries it. The role and database default is `0` (no limit), so "the budget" was a coin flip.
 *
 * Why not a startup `options=-c statement_timeout=...`: DigitalOcean's PgBouncer rejects it with
 * `unsupported startup parameter in options`.
 *
 * Why not one role setting: every service connects as the same role, so a role default cannot
 * give two pools two budgets, and any session SET still overrides it.
 *
 * What this does instead: each top-level statement runs in its own short transaction that opens
 * with `SET LOCAL statement_timeout`, which dies with the transaction and so cannot leak:
 *
 *     BEGIN; SET LOCAL statement_timeout = N     -- one simple-protocol round trip
 *     <the caller's statement>
 *     COMMIT                                     -- a failed statement turns this into ROLLBACK
 *
 * All three are queued on the client synchronously, so pg's FIFO query queue keeps the triple
 * together even when several callers share a client concurrently (PostGraphile does). The
 * caller's promise settles only after COMMIT, so a write is never reported done before it is.
 *
 * Per statement, not per request: one root field's failure must not abort its siblings (an
 * aborted transaction fails every later statement in it), and a request-wide transaction would
 * pin a PgBouncer server connection idle-in-transaction across event-loop gaps.
 *
 * A caller's own transaction (`BEGIN` / `START TRANSACTION`, e.g. drizzle `db.transaction`) is
 * passed through untouched; the wrapper adds a `SET LOCAL` right after the caller's BEGIN so the
 * pool budget applies there too, and the caller can still override it with its own SET LOCAL.
 * Statements inside the caller's transaction pass through until its COMMIT / ROLLBACK / END /
 * ABORT. `ROLLBACK TO SAVEPOINT` does not end it.
 */

import type {Pool, PoolClient} from "pg"

/** Accepts only a positive integer; anything else falls back, so a typo cannot turn the cap off. */
export function parseStatementBudgetMs(raw: string | undefined, fallbackMs: number): number {
	if (raw === undefined || raw.trim() === "") return fallbackMs
	const trimmed = raw.trim()
	if (!/^\d+$/.test(trimmed)) return fallbackMs
	const n = Number(trimmed)
	return Number.isSafeInteger(n) && n > 0 ? n : fallbackMs
}

const BEGINS_TRANSACTION = /^\s*(?:begin|start\s+transaction)\b/i
const ENDS_TRANSACTION = /^\s*(?:commit|end|abort|rollback)\b/i
const ROLLBACK_TO_SAVEPOINT = /^\s*rollback(?:\s+(?:work|transaction))?\s+to\b/i

function statementText(config: unknown): string | undefined {
	if (typeof config === "string") return config
	if (config && typeof config === "object" && typeof (config as {text?: unknown}).text === "string") {
		return (config as {text: string}).text
	}
	return undefined
}

export function beginsTransaction(text: string): boolean {
	return BEGINS_TRANSACTION.test(text)
}

export function endsTransaction(text: string): boolean {
	return ENDS_TRANSACTION.test(text) && !ROLLBACK_TO_SAVEPOINT.test(text)
}

/**
 * Called when the budget's own BEGIN / SET LOCAL failed but the caller's statement went on to
 * succeed — that statement ran without a budget. When the connection itself is broken the
 * statement fails too and the caller already sees that error, so this stays quiet.
 */
export type StatementBudgetFailureHandler = (error: unknown) => void

type QueryFn = (...args: unknown[]) => unknown
type Callback = (err: Error | null, res?: unknown) => void

const WRAPPED = Symbol.for("gaia.statementBudget.wrapped")

/**
 * Wrap one pg client's `query` so every top-level statement runs under `SET LOCAL
 * statement_timeout = budgetMs`. Idempotent per client.
 */
export function applyStatementBudget(
	client: PoolClient,
	budgetMs: number,
	onBudgetFailure?: StatementBudgetFailureHandler,
): void {
	if (!Number.isSafeInteger(budgetMs) || budgetMs <= 0) {
		throw new RangeError(`statement budget must be a positive integer of milliseconds, got ${budgetMs}`)
	}
	const target = client as unknown as {query: QueryFn; [WRAPPED]?: true}
	if (target[WRAPPED]) return
	target[WRAPPED] = true

	const original = target.query.bind(client) as QueryFn
	const setLocal = `SET LOCAL statement_timeout = ${budgetMs}`
	const openBudget = `BEGIN; ${setLocal}`
	// Mirrors the transaction state on the wire. Statements are queued synchronously and pg runs
	// its queue in order, so flipping this at enqueue time matches the order the server sees.
	let inCallerTransaction = false

	target.query = (...args: unknown[]): unknown => {
		const [config] = args
		// Submittables (cursors, query streams, pre-built Query objects) manage their own
		// lifecycle and callbacks; leave them alone.
		if (config && typeof config === "object" && typeof (config as {submit?: unknown}).submit === "function") {
			return original(...args)
		}
		const text = statementText(config)

		if (inCallerTransaction) {
			if (text !== undefined && endsTransaction(text)) inCallerTransaction = false
			return original(...args)
		}
		if (text !== undefined && beginsTransaction(text)) {
			inCallerTransaction = true
			const result = original(...args)
			// Queued behind the caller's BEGIN, ahead of anything the caller sends next. If the
			// BEGIN failed this lands outside a transaction, where SET LOCAL is a no-op warning.
			;(original(setLocal) as Promise<unknown>).catch(() => {})
			return result
		}
		if (text !== undefined && endsTransaction(text)) {
			// A stray COMMIT / ROLLBACK outside a transaction: nothing to wrap.
			return original(...args)
		}

		let values: unknown
		let callback: Callback | undefined
		if (typeof args[1] === "function") {
			callback = args[1] as Callback
		} else {
			values = args[1]
			if (typeof args[2] === "function") callback = args[2] as Callback
		}

		const begin = original(openBudget) as Promise<unknown>
		const statement = (values === undefined ? original(config) : original(config, values)) as Promise<unknown>
		const commit = original("COMMIT") as Promise<unknown>

		const settled = Promise.allSettled([begin, statement, commit]).then(([b, s, c]) => {
			if (s.status === "rejected") throw s.reason
			if (c.status === "rejected") throw c.reason
			if (b.status === "rejected") onBudgetFailure?.(b.reason)
			return s.value
		})

		if (callback) {
			const cb = callback
			settled.then(
				(res) => cb(null, res),
				(err: Error) => cb(err),
			)
			return undefined
		}
		return settled
	}
}

/**
 * Apply the budget to every client `pool` creates. Must be called before the pool hands out its
 * first client; pg-pool emits `connect` synchronously before the checkout callback runs.
 */
export function useStatementBudget(
	pool: Pool,
	budgetMs: number,
	onBudgetFailure?: StatementBudgetFailureHandler,
): void {
	if (!Number.isSafeInteger(budgetMs) || budgetMs <= 0) {
		throw new RangeError(`statement budget must be a positive integer of milliseconds, got ${budgetMs}`)
	}
	pool.on("connect", (client) => applyStatementBudget(client, budgetMs, onBudgetFailure))
}
