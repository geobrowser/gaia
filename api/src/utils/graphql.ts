import * as Effect from "effect/Effect"

class HttpError extends Error {
	readonly _tag = "HttpError"
}

class JsonParseError extends Error {
	readonly _tag = "JsonParseError"
}

class GraphqlRuntimeError extends Error {
	readonly _tag = "GraphqlRuntimeError"
}

interface GraphqlConfig {
	endpoint: string
	query: string
}

interface GraphqlResponse<T> {
	data: T
	errors: unknown[]
}

export function graphql<T>({endpoint, query}: GraphqlConfig) {
	const graphqlFetchEffect = Effect.tryPromise({
		try: () =>
			fetch(endpoint, {
				method: "POST",
				headers: {
					"Content-Type": "application/json",
				},
				body: JSON.stringify({query}),
			}),
		catch: (e) => {
			return new HttpError()
		},
	})

	return Effect.gen(function* (awaited) {
		const response = yield* awaited(Effect.retry(graphqlFetchEffect, {times: 3}))
		const json = yield* awaited(
			Effect.tryPromise({
				try: () => response.json() as Promise<GraphqlResponse<T>>,
				catch: () => new JsonParseError(),
			}),
		)

		if (json.errors?.length > 0) {
			return yield* awaited(
				Effect.fail(new GraphqlRuntimeError(json.errors.map((error) => JSON.stringify(error)).join(", "))),
			)
		}

		return json.data
	})
}
