import { swaggerUI } from "@hono/swagger-ui"
import { Effect, Either } from "effect"
import { Hono } from "hono"
import { openAPISpecs } from "hono-openapi"
import { compress } from "hono/compress"
import { cors } from "hono/cors"
import { health } from "./src/health"
import { graphqlServer } from "./src/kg/postgraphile"
import { createSearchRouter } from "./src/search"
import { createVersionedRouter } from "./src/versioned"
import { db } from "./src/services/storage/storage"
import { uploadEdit, uploadFile, uploadFileAlternativeGateway } from "./src/services/ipfs"
import { OpenSearchClient } from "./src/services/search"
import { runtime } from "./src/services/runtime"
import { requestId, canonicalRequestLogging } from "./src/middleware/requestLogging"

/**
 * Currently hand-rolling a compression polyfill until Bun implements
 * CompressionStream in the runtime.
 * https://github.com/oven-sh/bun/issues/1723
 */
import "./src/compression-polyfill"
import { log } from "./src/services/telemetry"

type AppEnv = {
	Variables: {
		requestId: string
		traceContext?: {
			traceId: string
			spanId: string
			traceFlags: number
		}
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
app.use("/search/*", canonicalRequestLogging())
app.use("/versioned/*", canonicalRequestLogging())
app.use("/graphql", canonicalRequestLogging())

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
	return graphqlServer.fetch(c.req.raw, {traceContext: c.get("traceContext")})
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
