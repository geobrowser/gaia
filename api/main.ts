import {Duration, Effect, Either, Layer, Schedule} from "effect"
import {Hono} from "hono"
import {cors} from "hono/cors"
import {graphqlServer} from "./src/kg/graphql-entry"
import {health} from "./src/health"
import {Environment, EnvironmentLive, make as makeEnvironment} from "./src/services/environment"
import {uploadEdit, uploadFile} from "./src/services/ipfs"
import {Storage, make as makeStorage} from "./src/services/storage/storage"
import {getPublishEditCalldata} from "./src/utils/calldata"
import {deploySpace} from "./src/utils/deploy-space"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

const app = new Hono()
app.use("*", cors())

app.route("/health", health)

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
	const {initialEditorAddress, spaceName, spaceEntityId, ops} = await c.req.json()

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
			spaceEntityId,
			ops,
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

	const providedDeploy = deployWithRetry.pipe(provideDeps)

	const result = await Effect.runPromise(
		Effect.either(providedDeploy).pipe(Effect.annotateLogs({editor: initialEditorAddress, spaceName})),
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

app.post("/space/:spaceId/edit/calldata", async (c) => {
	const {spaceId} = c.req.param()
	const {cid} = await c.req.json()

	if (!cid || !cid.startsWith("ipfs://")) {
		console.error(`[SPACE][calldata] Invalid CID ${cid}`)
		return new Response(
			JSON.stringify({
				error: "Missing required parameters",
				reason: "An IPFS CID prefixed with 'ipfs://' is required. e.g., ipfs://bafkreigkka6xfe3hb2tzcfqgm5clszs7oy7mct2awawivoxddcq6v3g5oi",
			}),
			{
				status: 400,
			},
		)
	}

	const getCalldata = Effect.gen(function* () {
		return yield* getPublishEditCalldata(spaceId, cid as string)
	})

	const calldata = await Effect.runPromise(Effect.either(getCalldata.pipe(provideDeps)))

	if (Either.isLeft(calldata)) {
		const error = calldata.left

		switch (error._tag) {
			case "ConfigError":
				console.error("[SPACE][calldata] Invalid server config")
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
					`[SPACE][calldata] Failed to generate calldata for edit. message: ${error.message} – cause: ${error.cause}`,
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
	}

	if (calldata.right === null) {
		return new Response(
			JSON.stringify({
				error: "Failed to generate calldata",
				reason: `Could not find space with id ${spaceId}. Ensure the space exists and that it's on the correct network. This API is associated with chain id ${EnvironmentLive.chainId}`,
			}),
			{
				status: 500,
			},
		)
	}

	return Response.json(calldata.right)
})

export default app
