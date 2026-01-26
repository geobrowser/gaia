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
import { requestId, canonicalRequestLogging } from "./src/middleware/requestLogging"

/**
 * Currently hand-rolling a compression polyfill until Bun implements
 * CompressionStream in the runtime.
 * https://github.com/oven-sh/bun/issues/1723
 */
import "./src/compression-polyfill"
import { log } from "./src/services/telemetry"
import { deployPersonalSpace } from "./src/space/deploy-personal-space"
import { deployPublicSpace } from "./src/space/deploy-public-space"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

const app = new Hono<AppEnv>()

// Request ID middleware (cheap, needed everywhere for correlation)
app.use("*", requestId())

app.use("*", cors())
app.use(
	compress({
		encoding: "gzip",
	}),
)

// Health routes - no tracing (high frequency, low value)
app.route("/health", health)

// Apply canonical logging/tracing to API routes (not health)
// Health checks are high-frequency noise with low observability value
app.use("/ipfs/*", canonicalRequestLogging())
app.use("/deploy", canonicalRequestLogging())
app.use("/deploy/*", canonicalRequestLogging())
app.use("/space/*", canonicalRequestLogging())
app.use("/search/*", canonicalRequestLogging())
app.use("/versioned/*", canonicalRequestLogging())
app.use("/graphql", canonicalRequestLogging())
app.use("/v2/graphql", canonicalRequestLogging())

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
	const requestId = c.get("requestId") ?? "unknown"

	const program = Effect.gen(function* () {
		if (!file) {
			yield* Effect.logWarning("No file provided")
			return yield* Effect.fail({_tag: "ValidationError" as const, status: 400, message: "No file provided"})
		}

		const result = yield* uploadEdit(file).pipe(
			Effect.mapError((error) => ({_tag: "UploadError" as const, status: 500, message: error.message})),
		)

		return result
	}).pipe(
		Effect.withSpan("/ipfs/upload-edit"),
		Effect.annotateLogs({requestId, fileName: file?.name, fileSize: file?.size}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => new Response(error.message, {status: error.status}),
		onRight: (data) => c.json({cid: data.cid}),
	})
})

app.post("/ipfs/upload-file", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined
	const requestId = c.get("requestId")

	const program = Effect.gen(function* () {
		if (!file) {
			yield* Effect.logWarning("No file provided")
			return yield* Effect.fail({_tag: "ValidationError" as const, status: 400, message: "No file provided"})
		}

		const result = yield* uploadFile(file).pipe(
			Effect.mapError((error) => ({_tag: "UploadError" as const, status: 500, message: error.message})),
		)

		return result
	}).pipe(
		Effect.withSpan("/ipfs/upload-file"),
		Effect.annotateLogs({requestId, fileName: file?.name, fileSize: file?.size}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => new Response(error.message, {status: error.status}),
		onRight: (data) => c.json({cid: data.cid}),
	})
})

app.post("/ipfs/upload-file-alternative-gateway", async (c) => {
	const formData = await c.req.formData()
	const file = formData.get("file") as File | undefined
	const requestId = c.get("requestId")

	const program = Effect.gen(function* () {
		if (!file) {
			yield* Effect.logWarning("No file provided")
			return yield* Effect.fail({_tag: "ValidationError" as const, status: 400, message: "No file provided"})
		}

		const result = yield* uploadFileAlternativeGateway(file).pipe(
			Effect.mapError((error) => ({_tag: "UploadError" as const, status: 500, message: error.message})),
		)

		return result
	}).pipe(
		Effect.withSpan("/ipfs/upload-file-alternative-gateway"),
		Effect.annotateLogs({requestId, fileName: file?.name, fileSize: file?.size}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) => new Response(error.message, {status: error.status}),
		onRight: (data) => c.json({cid: data.cid}),
	})
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
	const requestId = c.get("requestId") ?? "unknown"

	const program = Effect.gen(function* () {
		if (initialEditorAddress === null || spaceName === null) {
			yield* Effect.logWarning("Missing required parameters")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Missing required parameters",
				reason: "An initial editor account and space name are required to deploy a space.",
			})
		}

		const spaceId = yield* Effect.retry(
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
		).pipe(
			Effect.mapError((error) => ({
				_tag: "DeployError" as const,
				status: 500,
				message: error.message,
				reason: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
			})),
		)

		return spaceId
	}).pipe(
		Effect.withSpan("/deploy/personal"),
		Effect.annotateLogs({requestId, editor: initialEditorAddress, spaceName}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) =>
			new Response(JSON.stringify({error: error.message, reason: error.reason}), {status: error.status}),
		onRight: (spaceId) => Response.json({spaceId}),
	})
})

app.post("deploy/public", async (c) => {
	const {initialEditorAddresses, spaceName, spaceEntityId, ops} = await c.req.json()
	const requestId = c.get("requestId") ?? "unknown"

	const program = Effect.gen(function* () {
		if (initialEditorAddresses === null || spaceName === null) {
			yield* Effect.logWarning("Missing required parameters")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Missing required parameters",
				reason: "An initial editor account and space name are required to deploy a space.",
			})
		}

		if (initialEditorAddresses.length === 0) {
			yield* Effect.logWarning("Invalid parameter initialEditorAddresses")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Invalid parameter initialEditorAddresses",
				reason: "At least one valid account address is required to deploy a space.",
			})
		}

		const spaceId = yield* Effect.retry(
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
		).pipe(
			Effect.mapError((error) => ({
				_tag: "DeployError" as const,
				status: 500,
				message: error.message,
				reason: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
			})),
		)

		return spaceId
	}).pipe(
		Effect.withSpan("/deploy/public"),
		Effect.annotateLogs({requestId, editors: initialEditorAddresses, spaceName}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) =>
			new Response(JSON.stringify({error: error.message, reason: error.reason}), {status: error.status}),
		onRight: (spaceId) => Response.json({spaceId}),
	})
})

/**
 * The /deploy route is a legacy route for deploying PERSONAL spaces. Leaving it for
 * now until we're ready to deprecate it.
 */
app.post("deploy", async (c) => {
	const {initialEditorAddress, spaceName, spaceEntityId, ops} = await c.req.json()
	const requestId = c.get("requestId") ?? "unknown"

	const program = Effect.gen(function* () {
		if (initialEditorAddress === null || spaceName === null) {
			yield* Effect.logWarning("Missing required parameters")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Missing required parameters",
				reason: "An initial editor account and space name are required to deploy a space.",
			})
		}

		const spaceId = yield* Effect.retry(
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
		).pipe(
			Effect.mapError((error) => ({
				_tag: "DeployError" as const,
				status: 500,
				message: error.message,
				reason: `Failed to deploy space. message: ${error.message} – cause: ${error.cause}`,
			})),
		)

		return spaceId
	}).pipe(
		Effect.withSpan("/deploy"),
		Effect.annotateLogs({requestId, editor: initialEditorAddress, spaceName}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) =>
			new Response(JSON.stringify({error: error.message, reason: error.reason}), {status: error.status}),
		onRight: (spaceId) => Response.json({spaceId}),
	})
})

const CalldataRequestSchema = Schema.Struct({
	cid: Schema.String,
})

app.post("/space/:spaceId/edit/calldata", async (c) => {
	const {spaceId} = c.req.param()
	const maybeRequestJson = await c.req.json()
	const requestId = c.get("requestId") ?? "unknown"

	const program = Effect.gen(function* () {
		const parsedRequestJsonResult = Schema.decodeUnknownEither(CalldataRequestSchema)(maybeRequestJson)

		if (Either.isLeft(parsedRequestJsonResult)) {
			yield* Effect.logWarning("Invalid request json")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Missing required parameters",
				reason: "An IPFS CID prefixed with 'ipfs://' is required. e.g., ipfs://bafkreigkka6xfe3hb2tzcfqgm5clszs7oy7mct2awawivoxddcq6v3g5oi",
			})
		}

		const cid = parsedRequestJsonResult.right.cid

		if (!cid || !cid.startsWith("ipfs://")) {
			yield* Effect.logWarning("Invalid CID format")
			return yield* Effect.fail({
				_tag: "ValidationError" as const,
				status: 400,
				message: "Missing required parameters",
				reason: "An IPFS CID prefixed with 'ipfs://' is required. e.g., ipfs://bafkreigkka6xfe3hb2tzcfqgm5clszs7oy7mct2awawivoxddcq6v3g5oi",
			})
		}

		const calldata = yield* getPublishEditCalldata(spaceId, cid).pipe(
			Effect.mapError((error) => ({
				_tag: "CalldataError" as const,
				status: 500,
				message: error.message,
				reason: `Failed to generate calldata. message: ${error.message} – cause: ${error.cause}`,
			})),
		)

		if (calldata === null) {
			yield* Effect.logWarning("Space not found")
			return yield* Effect.fail({
				_tag: "NotFoundError" as const,
				status: 404,
				message: "Failed to generate calldata",
				reason: `Could not find space with id ${spaceId}. Ensure the space exists and that it's on the correct network. This API is associated with chain id ${EnvironmentLive.chainId}`,
			})
		}

		return calldata
	}).pipe(
		Effect.withSpan("/space/:spaceId/edit/calldata"),
		Effect.annotateLogs({requestId, spaceId}),
	)

	const result = await runtime.runPromise(Effect.either(program))

	return Either.match(result, {
		onLeft: (error) =>
			new Response(JSON.stringify({error: error.message, reason: error.reason}), {status: error.status}),
		onRight: (calldata) => Response.json(calldata),
	})
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
