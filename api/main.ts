import { swaggerUI } from "@hono/swagger-ui"
import { Duration, Effect, Either, Schedule, Schema } from "effect"
import { Hono } from "hono"
import { openAPISpecs } from "hono-openapi"
import { compress } from "hono/compress"
import { cors } from "hono/cors"
import { health } from "./src/health"
import { graphqlServer, graphqlServerV2 } from "./src/kg/postgraphile"
import { createSearchRouter } from "./src/search"
import { createVersionedRouter } from "./src/versioned"
import { db } from "./src/services/storage/storage"
import { EnvironmentLive } from "./src/services/environment"
import { uploadEdit, uploadFile, uploadFileAlternativeGateway } from "./src/services/ipfs"
import { OpenSearchClient } from "./src/services/search"
import { getPublishEditCalldata } from "./src/utils/calldata"
import { runtime } from "./src/services/runtime"

/**
 * Currently hand-rolling a compression polyfill until Bun implements
 * CompressionStream in the runtime.
 * https://github.com/oven-sh/bun/issues/1723
 */
import "./src/compression-polyfill"
import { log } from "./src/services/telemetry"
import { deployPersonalSpace } from "./src/space/deploy-personal-space"
import { deployPublicSpace } from "./src/space/deploy-public-space"

const app = new Hono()
app.use("*", cors())
app.use(
	compress({
		encoding: "gzip",
	}),
)

app.route("/health", health)

// Initialize search client with dependency injection
// Search is optional - if OPENSEARCH_URL is not set, search routes won't be added
const opensearchUrl = process.env.OPENSEARCH_URL
if (opensearchUrl) {
	// Validate URL format
	try {
		new URL(opensearchUrl)
	} catch (error) {
		log.error("Invalid OPENSEARCH_URL", {url: opensearchUrl})
		throw error
	}
	const searchClient = new OpenSearchClient(opensearchUrl)
	app.route("/search", createSearchRouter(searchClient, runtime))
	log.info("Search routes enabled", {url: opensearchUrl})
} else {
	log.info("Search routes disabled - OPENSEARCH_URL not set")
}

// Mount versioned entities router
app.route("/versioned", createVersionedRouter(db, runtime))
log.info("Versioned entity routes enabled")

app.get("/", swaggerUI({url: "/openapi"}))

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw)
})

app.use("/v2/graphql", async (c) => {
	return graphqlServerV2.fetch(c.req.raw)
})

app.post("/ipfs/upload-edit", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const program = uploadEdit(file).pipe(Effect.withSpan("/ipfs/upload-edit.uploadEdit"))
	const result = await runtime.runPromise(Effect.either(program))

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

	const program = uploadFile(file).pipe(Effect.withSpan("/ipfs/upload-file.uploadFile"))
	const result = await runtime.runPromise(Effect.either(program))

	if (Either.isLeft(result)) {
		// @TODO: Logging/tracing
		return new Response("Failed to upload file", {status: 500})
	}

	const cid = result.right.cid

	return c.json({cid})
})

app.post("/ipfs/upload-file-alternative-gateway", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined

	if (!file) {
		return new Response("No file provided", {status: 400})
	}

	const program = uploadFileAlternativeGateway(file).pipe(
		Effect.withSpan("/ipfs/upload-file-alternative-gateway.uploadFile"),
	)
	const result = await runtime.runPromise(Effect.either(program))

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
		log.error("Missing required parameters to deploy a space", {initialEditorAddress, spaceName})

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

	const program = Effect.retry(
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
		),
		{
			schedule: Schedule.exponential(Duration.millis(100)).pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.minutes(1))),
			),
			while: (error) => error._tag !== "WaitForSpaceToBeIndexedError",
		},
	).pipe(Effect.annotateLogs({editor: initialEditorAddress, spaceName}))

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => {
			log.error("Failed to deploy space", {
				route: "/deploy/personal",
				message: error.message,
				cause: String(error.cause),
			})

			return new Response(
				JSON.stringify({
					message: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
					reason: error.message,
				}),
				{
					status: 500,
				},
			)
		},
		onRight: (spaceId) => {
			return Response.json({spaceId})
		},
	})
})

app.post("deploy/public", async (c) => {
	const {initialEditorAddresses, spaceName, spaceEntityId, ops} = await c.req.json()

	if (initialEditorAddresses === null || spaceName === null) {
		log.error("Missing required parameters to deploy a space", {initialEditorAddresses, spaceName})

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
		log.error("Invalid parameter initialEditorAddresses", {
			reason: "At least one valid account address is required to deploy a space",
		})

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

	const program = Effect.retry(
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
		),
		{
			schedule: Schedule.exponential(Duration.millis(100)).pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.minutes(1))),
			),
			while: (error) => error._tag !== "WaitForSpaceToBeIndexedError",
		},
	).pipe(Effect.annotateLogs({editor: initialEditorAddresses, spaceName}))

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => {
			log.error("Failed to deploy space", {
				route: "/deploy/public",
				message: error.message,
				cause: String(error.cause),
			})

			return new Response(
				JSON.stringify({
					message: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
					reason: error.message,
				}),
				{
					status: 500,
				},
			)
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
app.post("deploy", async (c) => {
	const {initialEditorAddress, spaceName, spaceEntityId, ops} = await c.req.json()

	if (initialEditorAddress === null || spaceName === null) {
		log.error("Missing required parameters to deploy a space", {initialEditorAddress, spaceName})

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

	const program = Effect.retry(
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
		),
		{
			schedule: Schedule.exponential(Duration.millis(100)).pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.minutes(1))),
			),
			while: (error) => error._tag !== "WaitForSpaceToBeIndexedError",
		},
	).pipe(Effect.annotateLogs({editor: initialEditorAddress, spaceName}))

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => {
			log.error("Failed to deploy space", {
				route: "/deploy",
				message: error.message,
				cause: String(error.cause),
			})

			return new Response(
				JSON.stringify({
					message: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
					reason: error.message,
				}),
				{
					status: 500,
				},
			)
		},
		onRight: (spaceId) => {
			return Response.json({spaceId})
		},
	})
})

const CalldataRequestSchema = Schema.Struct({
	cid: Schema.String,
})

app.post("/space/:spaceId/edit/calldata", async (c) => {
	const {spaceId} = c.req.param()
	const maybeRequestJson = await c.req.json()

	const parsedRequestJsonResult = Schema.decodeUnknownEither(CalldataRequestSchema)(maybeRequestJson)

	if (Either.isLeft(parsedRequestJsonResult)) {
		log.error("Invalid request json", {route: "/space/:spaceId/edit/calldata", body: maybeRequestJson})

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
		log.error("Invalid CID", {route: "/space/:spaceId/edit/calldata", cid})
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

	const program = getPublishEditCalldata(spaceId, cid as string).pipe(
		Effect.withSpan("/space/:spaceId/edit/calldata.getCalldata"),
	)

	const calldata = await runtime.runPromise(Effect.either(program))

	if (Either.isLeft(calldata)) {
		const error = calldata.left

		log.error("Failed to generate calldata for edit", {
			route: "/space/:spaceId/edit/calldata",
			message: error.message,
			cause: String(error.cause),
		})

		return new Response(
			JSON.stringify({
				message: `Failed to generate calldata. message: ${error.message} – cause: ${error.cause}`,
				reason: error.message,
			}),
			{
				status: 500,
			},
		)
	}

	if (calldata.right === null) {
		log.error("Failed to generate calldata", {spaceId, reason: "Could not find space"})

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
