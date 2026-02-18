import {classifyDbFailure, isRetryableDbFailure} from "./dbFailures"

export type DbRetryConfig = {
	maxElapsedMs: number
	baseDelayMs: number
	maxDelayMs: number
}

export type DbRetryAttempt = {
	operationName: string
	attempt: number
	delayMs: number
	elapsedMs: number
	failureClass: string
	error: unknown
}

type DbRetryOptions = Partial<DbRetryConfig> & {
	operationName?: string
	onRetry?: (attempt: DbRetryAttempt) => void
	now?: () => number
	sleep?: (ms: number) => Promise<void>
	random?: () => number
}

function parseEnvInt(name: string, fallback: number): number {
	const raw = process.env[name]
	if (!raw) {
		return fallback
	}

	const parsed = Number.parseInt(raw, 10)
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return fallback
	}

	return parsed
}

function defaultSleep(ms: number): Promise<void> {
	return new Promise((resolve) => {
		setTimeout(resolve, ms)
	})
}

function normalizedConfig(overrides: Partial<DbRetryConfig>): DbRetryConfig {
	const baseConfig: DbRetryConfig = {
		maxElapsedMs: parseEnvInt("DB_RETRY_MAX_ELAPSED_MS", 3000),
		baseDelayMs: parseEnvInt("DB_RETRY_BASE_DELAY_MS", 100),
		maxDelayMs: parseEnvInt("DB_RETRY_MAX_DELAY_MS", 800),
	}

	const config: DbRetryConfig = {
		maxElapsedMs: overrides.maxElapsedMs ?? baseConfig.maxElapsedMs,
		baseDelayMs: overrides.baseDelayMs ?? baseConfig.baseDelayMs,
		maxDelayMs: overrides.maxDelayMs ?? baseConfig.maxDelayMs,
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

export async function withDbRetry<T>(operation: () => Promise<T>, options: DbRetryOptions = {}): Promise<T> {
	const config = normalizedConfig(options)
	const now = options.now ?? Date.now
	const sleep = options.sleep ?? defaultSleep
	const random = options.random ?? Math.random
	const operationName = options.operationName ?? "db.operation"

	const startedAt = now()
	let attempts = 0

	while (true) {
		attempts += 1

		try {
			return await operation()
		} catch (error) {
			if (!isRetryableDbFailure(error)) {
				throw error
			}

			const elapsedMs = now() - startedAt
			const delayMs = calculateRetryDelayMs(attempts, config, random)

			if (elapsedMs + delayMs > config.maxElapsedMs) {
				throw error
			}

			options.onRetry?.({
				operationName,
				attempt: attempts,
				delayMs,
				elapsedMs,
				failureClass: classifyDbFailure(error),
				error,
			})

			await sleep(delayMs)
		}
	}
}
