import {swaggerUI} from "@hono/swagger-ui"
import {Duration, Effect, Either, Layer, Schedule, Schema} from "effect"
import {Hono} from "hono"
import {compress} from "hono/compress"
import {cors} from "hono/cors"
import {openAPISpecs} from "hono-openapi"
import {health} from "./src/health"
import {graphqlServer} from "./src/kg/postgraphile"
import {Environment, EnvironmentLive, make as makeEnvironment} from "./src/services/environment"
import {uploadEdit, uploadFile} from "./src/services/ipfs"
import {make as makeStorage, Storage} from "./src/services/storage/storage"
import {getPublishEditCalldata} from "./src/utils/calldata"

/**
 * Currently hand-rolling a compression polyfill until Bun implements
 * CompressionStream in the runtime.
 * https://github.com/oven-sh/bun/issues/1723
 */
import "./src/compression-polyfill"
import {NodeSdkLive} from "./src/services/telemetry"
import {deployPersonalSpace} from "./src/space/deploy-personal-space"
import {deployPublicSpace} from "./src/space/deploy-public-space"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

const app = new Hono()
app.use("*", cors())
app.use(
	compress({
		encoding: "gzip",
	}),
)

app.route("/health", health)

app.get("/", swaggerUI({url: "/openapi"}))

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw)
})

app.post("/ipfs/upload-edit", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const result = await Effect.runPromise(
		Effect.either(uploadEdit(file)).pipe(
			Effect.withSpan("/ipfs/upload-edit.uploadEdit"),
			Effect.provide(EnvironmentLayer),
			Effect.provide(NodeSdkLive),
		),
	)

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

	const result = await Effect.runPromise(
		Effect.either(uploadFile(file)).pipe(
			Effect.withSpan("/ipfs/upload-file.uploadFile"),
			Effect.provide(EnvironmentLayer),
			Effect.provide(NodeSdkLive),
		),
	)

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

// const DeployParametersSchema = Schema.Struct({
// 	initialEditorAddresses: Schema.Array(Schema.StringFromHex),
// 	spaceName: Schema.String,
// 	ops: Schema.Array(Schema.Any),
// 	spaceEntityId: Schema.NullOr(Schema.String),
// 	governanceType: Schema.Union(Schema.Literal("PERSONAL"), Schema.Literal("PUBLIC")),
// })

// const DeployResponseSchema = Schema.Struct({
// 	spaceId: Schema.String,
// })

app.post("deploy/personal", async (c) => {
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
		deployPersonalSpace({
			initialEditorAddress,
			spaceName,
			spaceEntityId,
			ops,
		}).pipe(
			Effect.withSpan("/deploy/personal.deploySpace"),
			Effect.annotateSpans({
				initialEditorAddress,
				spaceName,
				spaceEntityId,
			}),
			Effect.provide(NodeSdkLive),
			Effect.provide(EnvironmentLayer),
		),
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

app.post("deploy/public", async (c) => {
	const {initialEditorAddresses, spaceName, spaceEntityId, ops} = await c.req.json()

	if (initialEditorAddresses === null || spaceName === null) {
		console.error(
			`[SPACE][deploy] Missing required parameters to deploy a space ${JSON.stringify({initialEditorAddresses, spaceName})}`,
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

	if (initialEditorAddresses.length === 0) {
		console.error(
			"[SPACE][deploy] Invalid parameter initialEditorAddresses. At least one valid account address is required to deploy a space.",
		)

		return new Response(
			JSON.stringify({
				error: "Invalid parameter initialEditorAddresses",
				reason: "Invalid parameter initialEditorAddresses. At least one valid account address is required to deploy a space.",
			}),
			{
				status: 400,
			},
		)
	}

	const deployWithRetry = Effect.retry(
		deployPublicSpace({
			initialEditorAddresses,
			spaceName,
			spaceEntityId,
			ops,
		}).pipe(
			Effect.withSpan("/deploy/public.deploySpace"),
			Effect.annotateSpans({
				initialEditorAddresses,
				spaceName,
				spaceEntityId,
			}),
			Effect.provide(NodeSdkLive),
			Effect.provide(EnvironmentLayer),
		),
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
		Effect.either(providedDeploy).pipe(Effect.annotateLogs({editor: initialEditorAddresses, spaceName})),
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

/**
 * The /deploy route is a legacy route for deploying PERSONAL spaces. Leaving it for
 * now until we're ready to deprecate it.
 */
app.post(
	"deploy",
	// describeRoute({
	// 	validateResponse: true,
	// 	description: "Deploys a space with the provided parameters",
	// requestBody: {
	// 	required: true,
	// 	content: {
	// 		"application/json": {
	// 			schema: DeployParametersSchema,
	// 		},
	// 	},
	// },
	// 	responses: {
	// 		200: {
	// 			description: "Successful space deployment",
	// 			content: {
	// 				"application/json": {schema: resolver(DeployResponseSchema)},
	// 			},
	// 		},
	// 		400: {
	// 			description:
	// 				"Missing required parameters. An initial editor account and space name are required to deploy a space.",
	// 		},
	// 	},
	// }),
	// effectValidator("json", DeployParametersSchema),
	async (c) => {
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
			deployPersonalSpace({
				initialEditorAddress,
				spaceName,
				spaceEntityId,
				ops,
			}).pipe(
				Effect.withSpan("/deploy.deploySpace"),
				Effect.annotateSpans({
					initialEditorAddress,
					spaceName,
					spaceEntityId,
				}),
				Effect.provide(NodeSdkLive),
				Effect.provide(EnvironmentLayer),
			),
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
	},
)

const CalldataRequestSchema = Schema.Struct({
	cid: Schema.String,
})

app.post("/space/:spaceId/edit/calldata", async (c) => {
	const {spaceId} = c.req.param()
	const maybeRequestJson = await c.req.json()

	const parsedRequestJsonResult = Schema.decodeUnknownEither(CalldataRequestSchema)(maybeRequestJson)

	if (Either.isLeft(parsedRequestJsonResult)) {
		console.error(`[SPACE][calldata] Invalid request json. ${maybeRequestJson}`)

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

	const cid = parsedRequestJsonResult.right.cid

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
		console.error(`Failed to generate calldata. Could not find space with id ${spaceId}.`)

		return new Response(
			JSON.stringify({
				error: "Failed to generate calldata",
				reason: `Could not find space with id ${spaceId}. Ensure the space exists and that it's on the correct network. This API is associated with chain id ${EnvironmentLive.chainId}`,
			}),
			{
				status: 404,
			},
		)
	}

	return Response.json(calldata.right)
})

app.get(
	"/openapi",
	openAPISpecs(app, {
		documentation: {
			info: {
				title: "Geo API",
				version: "1.0.0",
				description: "API for interacting with the Geo knowledge graph",
			},
			servers: [
				{url: "http://localhost:3000", description: "Local Server"},
				{url: "https://api-testnet.geobrowser.io", description: "Testnet Geo API"},
			],
		},
	}),
)

export default app
