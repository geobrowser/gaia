import type {PoolClient} from "pg"

import {withDbRetry} from "../services/dbRetry"

type Logger = {
	warn: (message: string, context: Record<string, unknown>) => void
}

type ConnectPgClientOptions = {
	pool: {connect: () => Promise<PoolClient>}
	operationName: string
	getPoolStats: () => Record<string, number>
	logger: Logger
}

export async function connectPgClientWithRetry({
	pool,
	operationName,
	getPoolStats,
	logger,
}: ConnectPgClientOptions): Promise<PoolClient> {
	return withDbRetry(() => pool.connect(), {
		operationName,
		onRetry: ({attempt, delayMs, elapsedMs, failureClass, error}) => {
			logger.warn("Retrying PostGraphile pool connect", {
				attempt,
				delayMs,
				elapsedMs,
				failureClass,
				operationName,
				error: error instanceof Error ? error.message : String(error),
				poolStats: getPoolStats(),
			})
		},
	})
}
