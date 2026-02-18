import {Duration, Effect, Ref, Schedule} from "effect"

import {classifyDbFailure, isRetryableDbFailureClass} from "./dbFailures"
import {parsePositiveIntEnv} from "./numberEnv"

export type DbRetryConfig = {
	maxElapsedMs: number
	baseDelayMs: number
	maxDelayMs: number
	maxAttempts: number
}

export type DbRetryAttempt = {
	operationName: string
	attempt: number
	delayMs: number
	elapsedMs: number
	failureClass: string
	error: unknown
}

export type DbRetryGiveUp = {
	operationName: string
	attempts: number
	elapsedMs: number
	failureClass: string
	error: unknown
	reason: "non_retryable" | "budget_exceeded" | "max_attempts"
}

type DbRetryOptions = Partial<DbRetryConfig> & {
	operationName?: string
	onRetry?: (attempt: DbRetryAttempt) => void
	onGiveUp?: (details: DbRetryGiveUp) => void
	now?: () => number
	random?: () => number
}

function normalizedConfig(overrides: Partial<DbRetryConfig>): DbRetryConfig {
	const baseConfig: DbRetryConfig = {
		maxElapsedMs: parsePositiveIntEnv("DB_RETRY_MAX_ELAPSED_MS", 12000),
		baseDelayMs: parsePositiveIntEnv("DB_RETRY_BASE_DELAY_MS", 100),
		maxDelayMs: parsePositiveIntEnv("DB_RETRY_MAX_DELAY_MS", 800),
		maxAttempts: parsePositiveIntEnv("DB_RETRY_MAX_ATTEMPTS", 3),
	}

	const config: DbRetryConfig = {
		maxElapsedMs: overrides.maxElapsedMs ?? baseConfig.maxElapsedMs,
		baseDelayMs: overrides.baseDelayMs ?? baseConfig.baseDelayMs,
		maxDelayMs: overrides.maxDelayMs ?? baseConfig.maxDelayMs,
		maxAttempts: overrides.maxAttempts ?? baseConfig.maxAttempts,
	}

	if (config.maxDelayMs < config.baseDelayMs) {
		config.maxDelayMs = config.baseDelayMs
	}

	return config
}

export function calculateRetryDelayMs(
	attempt: number,
	config: DbRetryConfig,
	random: () => number = Math.random,
): number {
	const exponent = Math.max(0, attempt - 1)
	const unjittered = Math.min(config.baseDelayMs * 2 ** exponent, config.maxDelayMs)
	const jittered = unjittered * (0.5 + random())
	return Math.max(1, Math.min(config.maxDelayMs, Math.round(jittered)))
}

function inferGiveUpReason(args: {
	retryable: boolean
	attempts: number
	maxAttempts: number
	elapsedMs: number
	maxElapsedMs: number
}): DbRetryGiveUp["reason"] {
	if (!args.retryable) {
		return "non_retryable"
	}

	if (args.attempts >= args.maxAttempts) {
		return "max_attempts"
	}

	if (args.elapsedMs >= args.maxElapsedMs) {
		return "budget_exceeded"
	}

	return "budget_exceeded"
}

export async function withDbRetry<T>(operation: () => Promise<T>, options: DbRetryOptions = {}): Promise<T> {
	const config = normalizedConfig(options)
	const now = options.now ?? Date.now
	const random = options.random ?? Math.random
	const operationName = options.operationName ?? "db.operation"
	const startedAt = now()

	const retrySchedule = Schedule.intersect(
		Schedule.recurs(config.maxAttempts - 1),
		Schedule.exponential(Duration.millis(config.baseDelayMs)).pipe(
			Schedule.jittered,
			Schedule.compose(Schedule.elapsed),
			Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.millis(config.maxElapsedMs))),
		),
	)

	const program = Effect.gen(function* () {
		const attemptRef = yield* Ref.make(0)

		const operationEffect = Effect.gen(function* () {
			const attempt = yield* Ref.updateAndGet(attemptRef, (n) => n + 1)

			return yield* Effect.tryPromise({
				try: operation,
				catch: (error) => error,
			}).pipe(
				Effect.tapError((error) =>
					Effect.sync(() => {
						const failureClass = classifyDbFailure(error)
						if (!isRetryableDbFailureClass(failureClass)) {
							return
						}

						if (attempt >= config.maxAttempts) {
							return
						}

						const elapsedMs = now() - startedAt
						if (elapsedMs >= config.maxElapsedMs) {
							return
						}

						options.onRetry?.({
							operationName,
							attempt,
							delayMs: calculateRetryDelayMs(attempt, config, random),
							elapsedMs,
							failureClass,
							error,
						})
					}),
				),
			)
		})

		const retried = Effect.retry(operationEffect, {
			schedule: retrySchedule,
			while: (error) => isRetryableDbFailureClass(classifyDbFailure(error)),
		})

		return yield* retried.pipe(
			Effect.tapError((error) =>
				Effect.gen(function* () {
					const attempts = yield* Ref.get(attemptRef)
					const elapsedMs = now() - startedAt
					const failureClass = classifyDbFailure(error)

					options.onGiveUp?.({
						operationName,
						attempts,
						elapsedMs,
						failureClass,
						error,
						reason: inferGiveUpReason({
							retryable: isRetryableDbFailureClass(failureClass),
							attempts,
							maxAttempts: config.maxAttempts,
							elapsedMs,
							maxElapsedMs: config.maxElapsedMs,
						}),
					})
				}),
			),
		)
	}).pipe(Effect.withSpan(`db.retry.${operationName}`))

	return Effect.runPromise(program)
}
