export type DbFailureClass =
	| "pool_connect_timeout"
	| "connection_closed_abort"
	| "connection_reset"
	| "too_many_connections"
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

function normalizeCode(value: unknown): string | null {
	const err = asErrorLike(value)
	if (typeof err.code === "string") {
		return err.code.toUpperCase()
	}
	if (typeof err.code === "number") {
		return String(err.code)
	}
	return null
}

export function detectDbFailureClass(error: unknown): Exclude<DbFailureClass, "unknown_db_failure"> | null {
	const message = normalizeMessage(error)
	const err = asErrorLike(error)
	const code = normalizeCode(error)

	if (message.includes("timeout exceeded when trying to connect")) {
		return "pool_connect_timeout"
	}

	if (message.includes("canceling statement due to statement timeout")) {
		return "statement_timeout"
	}

	if (message.includes("too many clients already") || message.includes("remaining connection slots are reserved")) {
		return "too_many_connections"
	}

	if (
		code === "ECONNRESET" ||
		code === "57P01" ||
		code === "57P02" ||
		code === "57P03" ||
		code === "08006" ||
		code === "08001"
	) {
		return "connection_reset"
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

export function isRetryableDbFailureClass(failureClass: DbFailureClass): boolean {
	return (
		failureClass === "pool_connect_timeout" ||
		failureClass === "connection_closed_abort" ||
		failureClass === "connection_reset"
	)
}

export function isRetryableDbFailure(error: unknown): boolean {
	return isRetryableDbFailureClass(classifyDbFailure(error))
}
