/**
 * Tells a shadow's Postgres (reference) failure apart from a geo-db failure, so a result of `failed` means geo-db and only geo-db.
 * Both api shadows run a reference SQL on Postgres and compare it with geo-db; a reference query that is cancelled by the
 * statement timeout says nothing about geo-db.
 */

/** SQLSTATE `query_canceled`, raised for "canceling statement due to statement timeout". */
export const STATEMENT_TIMEOUT_SQLSTATE = "57014"

/** A failure that came from the Postgres reference side of a shadow, wrapping the original error as `cause`. */
export class ReferenceSideError extends Error {
	readonly sqlState: string | undefined
	constructor(cause: unknown) {
		super(cause instanceof Error ? cause.message : String(cause), {cause})
		this.name = "ReferenceSideError"
		this.sqlState = sqlStateOf(cause)
	}
}

/** The Postgres SQLSTATE on an error (`err.code` from pg), looking through `cause` in case a layer wraps the driver error. */
export function sqlStateOf(error: unknown): string | undefined {
	let e: unknown = error
	for (let depth = 0; depth < 5 && typeof e === "object" && e !== null; depth++) {
		const code = (e as {code?: unknown}).code
		if (typeof code === "string" && /^[0-9A-Z]{5}$/.test(code)) return code
		e = (e as {cause?: unknown}).cause
	}
	return undefined
}

/** Await a Postgres reference read; any rejection leaves as a ReferenceSideError. */
export async function viaReference<T>(read: Promise<T>): Promise<T> {
	try {
		return await read
	} catch (error) {
		throw new ReferenceSideError(error)
	}
}

/** The metric result for a shadow error: a reference-side statement timeout, any other reference-side error, else geo-db's `failed`. */
export function failureResult(error: unknown): "reference_timeout" | "reference_failed" | "failed" {
	if (!(error instanceof ReferenceSideError)) return "failed"
	return error.sqlState === STATEMENT_TIMEOUT_SQLSTATE ? "reference_timeout" : "reference_failed"
}
