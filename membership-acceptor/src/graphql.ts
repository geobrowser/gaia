/**
 * Minimal GraphQL client for the Geo API.
 *
 * This is the shared primitive for BYO policies: a policy receives a
 * {@link GraphQLClient} and uses it to fetch whatever data its decision needs
 * (editor status, reputation, payment, …). Kept deliberately thin — POST a
 * query, surface HTTP and GraphQL-level errors, bound it with a timeout.
 */

export class GraphQLError extends Error {
	override readonly name = "GraphQLError"
}

export interface GraphQLClient {
	/**
	 * Execute `query` and return its `data`, typed as `T`. Throws {@link GraphQLError}
	 * on a network/HTTP failure, a timeout, or a non-empty GraphQL `errors` array.
	 */
	query<T>(query: string, variables?: Record<string, unknown>): Promise<T>
}

export interface GraphQLClientOptions {
	endpoint: string
	/** Abort the request after this many ms (default 10s). */
	timeoutMs?: number
	/** Extra headers (e.g. auth) merged onto the request. */
	headers?: Record<string, string>
}

const DEFAULT_TIMEOUT_MS = 10_000

export function createGraphQLClient(opts: GraphQLClientOptions): GraphQLClient {
	const timeoutMs = opts.timeoutMs ?? DEFAULT_TIMEOUT_MS

	return {
		async query<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
			const controller = new AbortController()
			const timer = setTimeout(() => controller.abort(), timeoutMs)

			let response: Response
			try {
				response = await fetch(opts.endpoint, {
					method: "POST",
					headers: {"content-type": "application/json", ...opts.headers},
					body: JSON.stringify({query, variables}),
					signal: controller.signal,
				})
			} catch (err) {
				throw new GraphQLError(`request failed: ${err instanceof Error ? err.message : String(err)}`)
			} finally {
				clearTimeout(timer)
			}

			if (!response.ok) {
				throw new GraphQLError(`HTTP ${response.status} ${response.statusText}`)
			}

			let body: {data?: T; errors?: Array<{message: string}>}
			try {
				body = (await response.json()) as typeof body
			} catch (err) {
				throw new GraphQLError(`invalid JSON response: ${err instanceof Error ? err.message : String(err)}`)
			}

			if (body.errors && body.errors.length > 0) {
				throw new GraphQLError(body.errors.map((e) => e.message).join("; "))
			}
			if (body.data === undefined || body.data === null) {
				throw new GraphQLError("response had no data")
			}
			return body.data
		},
	}
}
