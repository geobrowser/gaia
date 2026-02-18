import type {Pool, PoolClient} from "pg"

import type {DbRetryAttempt, DbRetryGiveUp} from "./dbRetry"
import {withDbRetry} from "./dbRetry"

type PoolConnectCallback = (
	error: Error | undefined,
	client: PoolClient | undefined,
	done: (release?: unknown) => void,
) => void

type PoolRetryOptions = {
	pool: Pool
	operationName: string
	onRetry: (attempt: DbRetryAttempt) => void
	onGiveUp: (details: DbRetryGiveUp) => void
}

export function createPoolWithRetryConnect({pool, operationName, onRetry, onGiveUp}: PoolRetryOptions): Pool {
	const connectWithRetry = () =>
		withDbRetry(() => pool.connect(), {
			operationName,
			onRetry,
			onGiveUp,
		})

	const connect = (callback?: PoolConnectCallback): Promise<PoolClient> | undefined => {
		if (!callback) {
			return connectWithRetry()
		}

		void connectWithRetry()
			.then((client) => {
				const done = (release?: unknown) => {
					client.release(release as never)
				}
				callback(undefined, client, done)
			})
			.catch((error) => {
				const normalized = error instanceof Error ? error : new Error(String(error))
				callback(normalized, undefined, () => {})
			})
	}

	return new Proxy(pool, {
		get(target, property, receiver) {
			if (property === "connect") {
				return connect
			}

			const value = Reflect.get(target, property, receiver)
			if (typeof value === "function") {
				return value.bind(target)
			}

			return value
		},
	})
}
