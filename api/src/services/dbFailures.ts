export type DbFailureClass =
	| "pool_connect_timeout"
	| "connection_closed_abort"
	| "statement_timeout"
	| "unknown_db_failure"

type ErrorLike = {
	name?: string
	message?: string
	code?: string | number
}

function asErrorLike(value: unknown): ErrorLike {
	if (value && typeof value === "object") {
		return value as ErrorLike
	}
	return {}
}

function normalizeMessage(value: unknown): string {
	if (value instanceof Error) {
		return value.message.toLowerCase()
	}
	const err = asErrorLike(value)
	if (typeof err.message === "string") {
		return err.message.toLowerCase()
	}
	return String(value).toLowerCase()
}

export function detectDbFailureClass(error: unknown): Exclude<DbFailureClass, "unknown_db_failure"> | null {
	const message = normalizeMessage(error)
	const err = asErrorLike(error)

	if (message.includes("timeout exceeded when trying to connect")) {
		return "pool_connect_timeout"
	}

	if (message.includes("canceling statement due to statement timeout")) {
		return "statement_timeout"
	}

	const isAbort =
		err.name === "AbortError" ||
		message.includes("aborterror") ||
		message.includes("the connection was closed") ||
		message.includes("connection terminated")

	if (isAbort) {
		return "connection_closed_abort"
	}

	return null
}

export function classifyDbFailure(error: unknown): DbFailureClass {
	return detectDbFailureClass(error) ?? "unknown_db_failure"
}

export function isPoolConnectTimeout(error: unknown): boolean {
	return classifyDbFailure(error) === "pool_connect_timeout"
}
