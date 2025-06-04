import {Duration, Effect, Either, Layer, Schedule} from "effect"
import {Hono} from "hono"
import {cors} from "hono/cors"
import {graphqlServer} from "./src/kg/graphql-entry"
import {Environment, make as makeEnvironment} from "./src/services/environment"
import {uploadEdit, uploadFile} from "./src/services/ipfs"
import {deploySpace} from "./src/utils/deploy-space"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)

const app = new Hono()
app.use("*", cors())

app.get("/health", (c) => {
	return c.json({healthy: true})
})

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw)
})

app.post("/ipfs/upload-edit", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const result = await Effect.runPromise(Effect.either(uploadEdit(file)).pipe(Effect.provide(EnvironmentLayer)))

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

app.post("/ipfs/upload-file", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const result = await Effect.runPromise(Effect.either(uploadFile(file)).pipe(Effect.provide(EnvironmentLayer)))

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

app.post("/deploy", async (c) => {
	const {initialEditorAddress, spaceName, network = "MAINNET"} = await c.req.json()

	if (initialEditorAddress === null || spaceName === null) {
		console.error(
			`[SPACE][deploy] Missing required parameters to deploy a space ${JSON.stringify({initialEditorAddress, spaceName})}`,
		)

		return new Response(
			JSON.stringify({
				error: "Missing required parameters",
				reason: "An initial editor account and space name are required to deploy a space.",
			}),
			{
				status: 400,
			},
		)
	}

	const deployWithRetry = Effect.retry(
		deploySpace({
			initialEditorAddress,
			spaceName,
			network,
		}).pipe(Effect.provide(EnvironmentLayer)),
		{
			schedule: Schedule.exponential(Duration.millis(100)).pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.minutes(1))),
			),
			while: (error) => error._tag !== "WaitForSpaceToBeIndexedError",
		},
	)

	const result = await Effect.runPromise(
		Effect.either(deployWithRetry).pipe(Effect.annotateLogs({editor: initialEditorAddress, spaceName})),
	)

	return Either.match(result, {
		onLeft: (error) => {
			switch (error._tag) {
				case "ConfigError":
					console.error("[SPACE][deploy] Invalid server config")
					return new Response(
						JSON.stringify({
							message: "Invalid server config. Please notify the server administrator.",
							reason: "Invalid server config. Please notify the server administrator.",
						}),
						{
							status: 500,
						},
					)
				default:
					console.error(
						`[SPACE][deploy] Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
					)

					return new Response(
						JSON.stringify({
							message: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
							reason: error.message,
						}),
						{
							status: 500,
						},
					)
			}
		},
		onRight: (spaceId) => {
			return Response.json({spaceId})
		},
	})
})

export default app
