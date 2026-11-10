import type {Pool, PoolClient} from "pg"
import {describe, expect, it, vi} from "vitest"
import {
	applyStatementBudget,
	beginsTransaction,
	endsTransaction,
	parseStatementBudgetMs,
	useStatementBudget,
} from "./statementBudget"

const OPEN = "BEGIN; SET LOCAL statement_timeout = 30000"

type Outcome = {ok: unknown} | {err: Error}

/**
 * A stand-in for a pg Client: records what was queued, in queue order, and settles each query
 * with a scripted outcome (default: a result naming the statement). Like pg, it answers with a
 * promise unless given a callback.
 */
function fakeClient(script: Record<string, Outcome> = {}) {
	const sent: {text: unknown; values?: unknown}[] = []
	const client = {
		query: vi.fn((config: unknown, values?: unknown, callback?: unknown) => {
			if (typeof values === "function") {
				callback = values
				values = undefined
			}
			const text = typeof config === "string" ? config : (config as {text: string}).text
			sent.push(values === undefined ? {text} : {text, values})
			const outcome = script[text] ?? {ok: {rows: [], command: text}}
			const p = "err" in outcome ? Promise.reject(outcome.err) : Promise.resolve(outcome.ok)
			if (typeof callback === "function") {
				p.then(
					(r) => (callback as (e: Error | null, r?: unknown) => void)(null, r),
					(e) => (callback as (e: Error) => void)(e),
				)
				return undefined
			}
			return p
		}),
	}
	return {client: client as unknown as PoolClient, sent, texts: () => sent.map((s) => s.text)}
}

describe("parseStatementBudgetMs", () => {
	it("takes a positive integer", () => {
		expect(parseStatementBudgetMs("45000", 30000)).toBe(45000)
		expect(parseStatementBudgetMs(" 1 ", 30000)).toBe(1)
	})

	it("falls back on anything that would weaken or disable the cap", () => {
		for (const raw of [undefined, "", "  ", "0", "-5", "1.5", "30s", "abc", "1e9", "99999999999999999999"]) {
			expect(parseStatementBudgetMs(raw, 30000)).toBe(30000)
		}
	})
})

describe("transaction boundary detection", () => {
	it("recognises a caller opening a transaction", () => {
		for (const t of ["BEGIN", "begin read only", "  BEGIN ISOLATION LEVEL SERIALIZABLE", "start transaction"]) {
			expect(beginsTransaction(t)).toBe(true)
		}
		for (const t of ["SELECT 1", "beginning", "WITH begin AS (SELECT 1) SELECT * FROM begin"]) {
			expect(beginsTransaction(t)).toBe(false)
		}
	})

	it("recognises a caller ending one, but not rolling back to a savepoint", () => {
		for (const t of ["COMMIT", "commit", "END", "ABORT", "ROLLBACK", "rollback transaction"]) {
			expect(endsTransaction(t)).toBe(true)
		}
		for (const t of [
			"ROLLBACK TO SAVEPOINT sp1",
			"rollback to sp1",
			"ROLLBACK TRANSACTION TO SAVEPOINT a",
			"SAVEPOINT a",
		]) {
			expect(endsTransaction(t)).toBe(false)
		}
	})
})

describe("applyStatementBudget", () => {
	it("runs a statement as BEGIN + SET LOCAL, the statement, COMMIT — and returns the statement's result", async () => {
		const {client, sent} = fakeClient({"SELECT $1": {ok: {rows: [{x: 1}]}}})
		applyStatementBudget(client, 30000)

		const res = await client.query("SELECT $1", [1])

		expect(res).toEqual({rows: [{x: 1}]})
		expect(sent).toEqual([{text: OPEN}, {text: "SELECT $1", values: [1]}, {text: "COMMIT"}])
	})

	it("passes a config object through unchanged", async () => {
		const {client, sent} = fakeClient()
		applyStatementBudget(client, 30000)
		const config = {text: "SELECT 2", values: [], rowMode: "array" as const}

		await client.query(config)

		expect(sent.map((s) => s.text)).toEqual([OPEN, "SELECT 2", "COMMIT"])
	})

	it("queues all three synchronously, so concurrent callers on one client never interleave", async () => {
		const {client, texts} = fakeClient()
		applyStatementBudget(client, 30000)

		const a = client.query("SELECT 'a'")
		const b = client.query("SELECT 'b'")
		// Everything is on the queue before either settles.
		expect(texts()).toEqual([OPEN, "SELECT 'a'", "COMMIT", OPEN, "SELECT 'b'", "COMMIT"])
		await Promise.all([a, b])
	})

	it("rejects with the statement's error, and still sends COMMIT to close the aborted transaction", async () => {
		const boom = Object.assign(new Error("canceling statement due to statement timeout"), {code: "57014"})
		const onFailure = vi.fn()
		const {client, texts} = fakeClient({"SELECT pg_sleep(60)": {err: boom}})
		applyStatementBudget(client, 30000, onFailure)

		await expect(client.query("SELECT pg_sleep(60)")).rejects.toBe(boom)
		expect(texts()).toEqual([OPEN, "SELECT pg_sleep(60)", "COMMIT"])
		expect(onFailure).not.toHaveBeenCalled()
	})

	it("does not report a write as done until its COMMIT has succeeded", async () => {
		const lost = new Error("Connection terminated unexpectedly")
		const {client} = fakeClient({COMMIT: {err: lost}})
		applyStatementBudget(client, 30000)

		await expect(client.query("UPDATE t SET x = 1")).rejects.toBe(lost)
	})

	it("reports a statement that succeeded without its budget", async () => {
		const setFailed = new Error("SET failed")
		const onFailure = vi.fn()
		const {client} = fakeClient({[OPEN]: {err: setFailed}})
		applyStatementBudget(client, 30000, onFailure)

		await expect(client.query("SELECT 1")).resolves.toBeDefined()
		expect(onFailure).toHaveBeenCalledTimes(1)
		expect(onFailure).toHaveBeenCalledWith(setFailed)
	})

	it("stays quiet about the budget when the statement failed too (the caller already has that error)", async () => {
		const down = new Error("Client has encountered a connection error and is not queryable")
		const onFailure = vi.fn()
		const {client} = fakeClient({[OPEN]: {err: down}, "SELECT 1": {err: down}})
		applyStatementBudget(client, 30000, onFailure)

		await expect(client.query("SELECT 1")).rejects.toBe(down)
		expect(onFailure).not.toHaveBeenCalled()
	})

	it("supports pg-pool's callback form, (text, values, cb) and (text, cb)", async () => {
		const {client, sent} = fakeClient({"SELECT $1": {ok: "with-values"}, "SELECT 3": {ok: "no-values"}})
		applyStatementBudget(client, 30000)
		const query = client.query as unknown as (...a: unknown[]) => unknown

		const withValues = await new Promise((resolve, reject) => {
			const r = query("SELECT $1", [1], (err: Error | null, res: unknown) => (err ? reject(err) : resolve(res)))
			expect(r).toBeUndefined()
		})
		const noValues = await new Promise((resolve, reject) => {
			query("SELECT 3", (err: Error | null, res: unknown) => (err ? reject(err) : resolve(res)))
		})

		expect(withValues).toBe("with-values")
		expect(noValues).toBe("no-values")
		expect(sent).toEqual([
			{text: OPEN},
			{text: "SELECT $1", values: [1]},
			{text: "COMMIT"},
			{text: OPEN},
			{text: "SELECT 3"},
			{text: "COMMIT"},
		])
	})

	it("hands a statement error to the callback", async () => {
		const boom = new Error("boom")
		const {client} = fakeClient({"SELECT 1": {err: boom}})
		applyStatementBudget(client, 30000)
		const query = client.query as unknown as (...a: unknown[]) => unknown

		const err = await new Promise((resolve) => query("SELECT 1", [], (e: Error | null) => resolve(e)))
		expect(err).toBe(boom)
	})

	it("leaves a caller's own transaction alone, adding only the pool budget after its BEGIN", async () => {
		const {client, texts} = fakeClient()
		applyStatementBudget(client, 30000)

		await client.query("begin read only")
		await client.query("SET LOCAL statement_timeout = 300000")
		await client.query("SELECT 1")
		await client.query("savepoint sp1")
		await client.query("rollback to savepoint sp1")
		await client.query("SELECT 2")
		await client.query("commit")
		await client.query("SELECT 3")

		expect(texts()).toEqual([
			"begin read only",
			"SET LOCAL statement_timeout = 30000",
			"SET LOCAL statement_timeout = 300000",
			"SELECT 1",
			"savepoint sp1",
			"rollback to savepoint sp1",
			"SELECT 2",
			"commit",
			// Back outside the caller's transaction: wrapped again.
			OPEN,
			"SELECT 3",
			"COMMIT",
		])
	})

	it("sees a ROLLBACK as the end of the caller's transaction", async () => {
		const {client, texts} = fakeClient()
		applyStatementBudget(client, 30000)

		await client.query({text: "BEGIN"})
		await client.query({text: "ROLLBACK"})
		await client.query("SELECT 1")

		expect(texts()).toEqual([
			"BEGIN",
			"SET LOCAL statement_timeout = 30000",
			"ROLLBACK",
			OPEN,
			"SELECT 1",
			"COMMIT",
		])
	})

	it("passes a stray COMMIT outside any transaction straight through", async () => {
		const {client, texts} = fakeClient()
		applyStatementBudget(client, 30000)

		await client.query("COMMIT")
		expect(texts()).toEqual(["COMMIT"])
	})

	it("passes submittables (cursors, streams) straight through", () => {
		const {client, sent} = fakeClient()
		const inner = (client as unknown as {query: ReturnType<typeof vi.fn>}).query
		applyStatementBudget(client, 30000)
		const cursor = {submit: vi.fn(), text: "SELECT * FROM big"}

		client.query(cursor as never)

		expect(inner).toHaveBeenCalledTimes(1)
		expect(inner).toHaveBeenCalledWith(cursor)
		expect(sent).toHaveLength(1)
	})

	it("wraps a client only once", async () => {
		const {client, texts} = fakeClient()
		applyStatementBudget(client, 30000)
		applyStatementBudget(client, 1000)

		await client.query("SELECT 1")
		expect(texts()).toEqual([OPEN, "SELECT 1", "COMMIT"])
	})

	it("refuses a budget that is not a positive integer", () => {
		for (const bad of [0, -1, 1.5, Number.NaN]) {
			expect(() => applyStatementBudget(fakeClient().client, bad)).toThrow(RangeError)
		}
	})
})

describe("useStatementBudget", () => {
	it("wraps every client the pool connects", async () => {
		let onConnect: ((c: PoolClient) => void) | undefined
		const pool = {
			on: vi.fn((event: string, fn: (c: PoolClient) => void) => {
				if (event === "connect") onConnect = fn
			}),
		}
		useStatementBudget(pool as unknown as Pool, 30000)

		const {client, texts} = fakeClient()
		onConnect?.(client)
		await client.query("SELECT 1")

		expect(pool.on).toHaveBeenCalledWith("connect", expect.any(Function))
		expect(texts()).toEqual([OPEN, "SELECT 1", "COMMIT"])
	})

	it("refuses a bad budget at wiring time, not on first connect", () => {
		const pool = {on: vi.fn()}
		expect(() => useStatementBudget(pool as unknown as Pool, 0)).toThrow(RangeError)
		expect(pool.on).not.toHaveBeenCalled()
	})
})
