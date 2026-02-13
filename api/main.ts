import {swaggerUI} from "@hono/swagger-ui"
import {Effect, Either} from "effect"
import {Hono} from "hono"
import {cors} from "hono/cors"
import {describeRoute, openAPISpecs} from "hono-openapi"
import {health} from "./src/health"
import {graphqlServer} from "./src/kg/postgraphile"
import {canonicalRequestLogging, requestId} from "./src/middleware/requestLogging"
import {createProfileRouter} from "./src/profile"
import {createProposalsRouter} from "./src/proposals"
import {createSearchRouter} from "./src/search"
import {uploadEdit, uploadFile} from "./src/services/ipfs"
import {runtime} from "./src/services/runtime"
import {OpenSearchClient} from "./src/services/search"
import {db} from "./src/services/storage/storage"
import {log} from "./src/services/telemetry"
import {createVersionedRouter} from "./src/versioned"

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
log.info("HTTP compression disabled in API (managed by ingress)")

// Health routes - no tracing (high frequency, low value)
app.route("/health", health)

// Debug endpoint for heap profiling — only registered when ENABLE_DEBUG_ENDPOINTS is set.
// Usage: kubectl exec -n api <pod> -- curl -s localhost:3000/debug/heap-snapshot
//        kubectl cp api/<pod>:/tmp/heap-<ts>.heapsnapshot ./heap.heapsnapshot
// Open .heapsnapshot files in Chrome DevTools → Memory tab.
if (process.env.ENABLE_DEBUG_ENDPOINTS) {
	app.get("/debug/heap-snapshot", async (c) => {
		const snapshot = Bun.generateHeapSnapshot()
		const filename = `heap-${Date.now()}.heapsnapshot`
		const path = `/tmp/${filename}`
		await Bun.write(path, JSON.stringify(snapshot))
		log.info("Heap snapshot written", {path})
		return c.json({path, filename})
	})

	app.post("/debug/gc", (c) => {
		const fs = require("fs")
		const beforeStatus = fs.readFileSync("/proc/1/status", "utf8")
		const before = {
			rss: beforeStatus.match(/VmRSS:\s+(\d+)/)?.[1],
			anon: beforeStatus.match(/RssAnon:\s+(\d+)/)?.[1],
		}

		Bun.gc(true)

		const afterStatus = fs.readFileSync("/proc/1/status", "utf8")
		const after = {
			rss: afterStatus.match(/VmRSS:\s+(\d+)/)?.[1],
			anon: afterStatus.match(/RssAnon:\s+(\d+)/)?.[1],
		}

		const result = {
			before: {rssKB: Number(before.rss), anonKB: Number(before.anon)},
			after: {rssKB: Number(after.rss), anonKB: Number(after.anon)},
			freedKB: {
				rss: Number(before.rss) - Number(after.rss),
				anon: Number(before.anon) - Number(after.anon),
			},
		}
		log.info("GC triggered", result)
		return c.json(result)
	})

	log.info("Debug endpoints enabled")
}

// Apply canonical logging/tracing to API routes (not health)
// Health checks are high-frequency noise with low observability value
app.use("/ipfs/*", canonicalRequestLogging())
app.use("/profile/*", canonicalRequestLogging())
app.use("/search/*", canonicalRequestLogging())
app.use("/versioned/*", canonicalRequestLogging())
app.use("/proposals/*", canonicalRequestLogging())
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

	// Get environment-specific index name
	// staging -> staging_entities, production -> entities
	const environment = process.env.ENVIRONMENT
	const baseIndexAlias = process.env.INDEX_ALIAS ?? "entities"
	const indexName = environment === "staging" ? `staging_${baseIndexAlias}` : baseIndexAlias

	const searchClient = new OpenSearchClient(opensearchUrl, indexName)
	app.route("/search", createSearchRouter(searchClient, runtime))
	log.info("Search routes enabled", {url: opensearchUrl, indexName, environment: environment ?? "production"})
} else {
	log.info("Search routes disabled - OPENSEARCH_URL not set")
}

// Mount versioned entities router
app.route("/versioned", createVersionedRouter(db, runtime))
log.info("Versioned entity routes enabled")

// Mount profile router
app.route("/profile", createProfileRouter(db, runtime))
log.info("Profile routes enabled")

// Mount proposals router
app.route("/proposals", createProposalsRouter(db, runtime))
log.info("Proposals routes enabled")

app.get("/", swaggerUI({url: "/openapi"}))

app.use("/graphql", async (c) => {
	return graphqlServer.fetch(c.req.raw, {
		traceContext: c.get("traceContext"),
	})
})

app.post(
	"/ipfs/upload-edit",
	describeRoute({
		tags: ["IPFS"],
		summary: "Upload an edit to IPFS",
		description: "Uploads an edit file to IPFS and returns the content identifier (CID)",
		requestBody: {
			content: {
				"multipart/form-data": {
					schema: {
						type: "object",
						properties: {
							file: {
								type: "string",
								format: "binary",
								description: "The edit file to upload",
							},
						},
						required: ["file"],
					},
				},
			},
		},
		responses: {
			200: {
				description: "File successfully uploaded to IPFS",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								cid: {
									type: "string",
									description: "The IPFS content identifier (CID) of the uploaded file",
								},
							},
							required: ["cid"],
						},
					},
				},
			},
			400: {
				description: "No file provided",
				content: {
					"text/plain": {
						schema: {
							type: "string",
							example: "No file provided",
						},
					},
				},
			},
			500: {
				description: "Upload failed",
				content: {
					"text/plain": {
						schema: {
							type: "string",
						},
					},
				},
			},
		},
	}),
	async (c) => {
		const formData = await c.req.formData()
		const file = formData.get("file") as File | undefined
		const requestId = c.get("requestId") ?? "unknown"

		const program = Effect.gen(function* () {
			if (!file) {
				yield* Effect.logWarning("No file provided")
				return yield* Effect.fail({
					_tag: "ValidationError" as const,
					status: 400,
					message: "No file provided",
				})
			}

			const result = yield* uploadEdit(file).pipe(
				Effect.mapError((error) => ({
					_tag: "UploadError" as const,
					status: 500,
					message: error.message,
				})),
			)

			return result
		}).pipe(
			Effect.withSpan("/ipfs/upload-edit"),
			Effect.annotateLogs({
				requestId,
				fileName: file?.name,
				fileSize: file?.size,
			}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error) => new Response(error.message, {status: error.status}),
			onRight: (data) => c.json({cid: data.cid}),
		})
	},
)

app.post(
	"/ipfs/upload-file",
	describeRoute({
		tags: ["IPFS"],
		summary: "Upload a file to IPFS",
		description: "Uploads a file to IPFS and returns the content identifier (CID)",
		requestBody: {
			content: {
				"multipart/form-data": {
					schema: {
						type: "object",
						properties: {
							file: {
								type: "string",
								format: "binary",
								description: "The file to upload",
							},
						},
						required: ["file"],
					},
				},
			},
		},
		responses: {
			200: {
				description: "File successfully uploaded to IPFS",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								cid: {
									type: "string",
									description: "The IPFS content identifier (CID) of the uploaded file",
								},
							},
							required: ["cid"],
						},
					},
				},
			},
			400: {
				description: "No file provided",
				content: {
					"text/plain": {
						schema: {
							type: "string",
							example: "No file provided",
						},
					},
				},
			},
			500: {
				description: "Upload failed",
				content: {
					"text/plain": {
						schema: {
							type: "string",
						},
					},
				},
			},
		},
	}),
	async (c) => {
		const formData = await c.req.formData()
		const file = formData.get("file") as File | undefined
		const requestId = c.get("requestId")

		const program = Effect.gen(function* () {
			if (!file) {
				yield* Effect.logWarning("No file provided")
				return yield* Effect.fail({
					_tag: "ValidationError" as const,
					status: 400,
					message: "No file provided",
				})
			}

			const result = yield* uploadFile(file).pipe(
				Effect.mapError((error) => ({
					_tag: "UploadError" as const,
					status: 500,
					message: error.message,
				})),
			)

			return result
		}).pipe(
			Effect.withSpan("/ipfs/upload-file"),
			Effect.annotateLogs({
				requestId,
				fileName: file?.name,
				fileSize: file?.size,
			}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error) => new Response(error.message, {status: error.status}),
			onRight: (data) => c.json({cid: data.cid}),
		})
	},
)

// Backwards compatibility alias - uses same implementation as /ipfs/upload-file
app.post(
	"/ipfs/upload-file-alternative-gateway",
	describeRoute({
		tags: ["IPFS"],
		summary: "Upload a file to IPFS (deprecated)",
		description:
			"Deprecated: Use /ipfs/upload-file instead. This endpoint is maintained for backwards compatibility.",
		deprecated: true,
		requestBody: {
			content: {
				"multipart/form-data": {
					schema: {
						type: "object",
						properties: {
							file: {
								type: "string",
								format: "binary",
								description: "The file to upload",
							},
						},
						required: ["file"],
					},
				},
			},
		},
		responses: {
			200: {
				description: "File successfully uploaded to IPFS",
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								cid: {
									type: "string",
									description: "The IPFS content identifier (CID) of the uploaded file",
								},
							},
							required: ["cid"],
						},
					},
				},
			},
			400: {
				description: "No file provided",
				content: {
					"text/plain": {
						schema: {
							type: "string",
							example: "No file provided",
						},
					},
				},
			},
			500: {
				description: "Upload failed",
				content: {
					"text/plain": {
						schema: {
							type: "string",
						},
					},
				},
			},
		},
	}),
	async (c) => {
		const formData = await c.req.formData()
		const file = formData.get("file") as File | undefined
		const requestId = c.get("requestId")

		const program = Effect.gen(function* () {
			if (!file) {
				yield* Effect.logWarning("No file provided")
				return yield* Effect.fail({
					_tag: "ValidationError" as const,
					status: 400,
					message: "No file provided",
				})
			}

			const result = yield* uploadFile(file).pipe(
				Effect.mapError((error) => ({
					_tag: "UploadError" as const,
					status: 500,
					message: error.message,
				})),
			)

			return result
		}).pipe(
			Effect.withSpan("/ipfs/upload-file-alternative-gateway"),
			Effect.annotateLogs({
				requestId,
				fileName: file?.name,
				fileSize: file?.size,
			}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error) => new Response(error.message, {status: error.status}),
			onRight: (data) => c.json({cid: data.cid}),
		})
	},
)

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
				{
					url: "https://api-testnet.geobrowser.io",
					description: "Testnet Geo API",
				},
			],
			components: {
				schemas: {
					// Profile types
					Profile: {
						type: "object",
						description: "A user profile derived from their personal space",
						properties: {
							spaceId: {
								type: "string",
								format: "uuid",
								description: "The user's personal space ID",
							},
							name: {
								type: "string",
								nullable: true,
								description: "Display name from the NAME_PROPERTY value",
							},
							avatarUrl: {
								type: "string",
								nullable: true,
								description: "Avatar image URL from the AVATAR_PROPERTY relation",
							},
							address: {
								type: "string",
								description: "The user's wallet address (0x prefixed)",
							},
						},
						required: ["spaceId", "address"],
					},
					// Value types
					VersionedValue: {
						type: "object",
						description: "A value at a specific version. Only one value field will be set.",
						properties: {
							propertyId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							// Value columns (GRC-20 v2 data types) - only one will be set
							boolean: {type: "boolean", nullable: true},
							integer: {type: "integer", nullable: true},
							float: {type: "number", nullable: true},
							decimal: {type: "string", nullable: true},
							text: {type: "string", nullable: true},
							bytes: {
								type: "string",
								nullable: true,
								description: "Base64 encoded",
							},
							date: {type: "string", format: "date", nullable: true},
							time: {
								type: "string",
								nullable: true,
								description: "ISO 8601 time",
							},
							datetime: {type: "string", format: "date-time", nullable: true},
							schedule: {
								type: "object",
								nullable: true,
								description: "RFC 5545 schedule",
							},
							point: {
								type: "string",
								nullable: true,
								description: "WGS84 point",
							},
							rect: {
								type: "string",
								nullable: true,
								description: "WGS84 bounding box",
							},
							embedding: {type: "object", nullable: true},
							// Metadata
							language: {type: "string", nullable: true},
							unit: {type: "string", nullable: true},
							// Context metadata
							contextRootId: {type: "string", format: "uuid", nullable: true},
							contextEdgeTypeId: {
								type: "string",
								format: "uuid",
								nullable: true,
							},
						},
						required: ["propertyId", "spaceId"],
					},
					VersionedRelation: {
						type: "object",
						description: "A relation at a specific version (excluding block relations)",
						properties: {
							relationId: {type: "string", format: "uuid"},
							typeId: {type: "string", format: "uuid"},
							fromEntityId: {type: "string", format: "uuid"},
							fromSpaceId: {type: "string", format: "uuid", nullable: true},
							toEntityId: {type: "string", format: "uuid"},
							toSpaceId: {type: "string", format: "uuid", nullable: true},
							position: {type: "string", nullable: true},
							spaceId: {type: "string", format: "uuid"},
							verified: {type: "boolean", nullable: true},
							contextRootId: {type: "string", format: "uuid", nullable: true},
							contextEdgeTypeId: {
								type: "string",
								format: "uuid",
								nullable: true,
							},
						},
						required: ["relationId", "typeId", "fromEntityId", "toEntityId", "spaceId"],
					},
					BlockSnapshot: {
						type: "object",
						description: "A block snapshot - an entity linked via BLOCKS relation",
						properties: {
							id: {type: "string", format: "uuid"},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedValue"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedRelation"},
							},
						},
						required: ["id", "values", "relations"],
					},
					EntitySnapshot: {
						type: "object",
						description: "An entity snapshot at a specific version",
						properties: {
							id: {type: "string", format: "uuid"},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedValue"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedRelation"},
								description: "Excludes block relations",
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockSnapshot"},
							},
						},
						required: ["id", "values", "relations", "blocks"],
					},
					VersionEntry: {
						type: "object",
						description: "A version entry for listing versions",
						properties: {
							editId: {type: "string", format: "uuid"},
							blockNumber: {type: "string"},
							createdAt: {type: "string", format: "date-time"},
						},
						required: ["editId", "blockNumber", "createdAt"],
					},
					// Diff types
					DiffChunk: {
						type: "object",
						description: "A single chunk in a text diff",
						properties: {
							value: {type: "string"},
							added: {type: "boolean"},
							removed: {type: "boolean"},
						},
						required: ["value"],
					},
					ValueChange: {
						type: "object",
						description: "A value change with before/after values",
						properties: {
							propertyId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							type: {
								type: "string",
								enum: [
									"TEXT",
									"BOOL",
									"INT64",
									"FLOAT64",
									"DECIMAL",
									"BYTES",
									"DATE",
									"TIME",
									"DATETIME",
									"SCHEDULE",
									"POINT",
									"RECT",
									"EMBEDDING",
								],
							},
							before: {type: "string", nullable: true},
							after: {type: "string", nullable: true},
							diff: {
								type: "array",
								items: {$ref: "#/components/schemas/DiffChunk"},
								description: "Only present for TEXT type",
							},
						},
						required: ["propertyId", "spaceId", "type"],
					},
					RelationChange: {
						type: "object",
						description: "A relation change",
						properties: {
							relationId: {type: "string", format: "uuid"},
							typeId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							changeType: {type: "string", enum: ["ADD", "REMOVE", "UPDATE"]},
							before: {
								type: "object",
								nullable: true,
								properties: {
									toEntityId: {type: "string", format: "uuid"},
									toSpaceId: {type: "string", format: "uuid", nullable: true},
									position: {type: "string", nullable: true},
								},
								required: ["toEntityId"],
							},
							after: {
								type: "object",
								nullable: true,
								properties: {
									toEntityId: {type: "string", format: "uuid"},
									toSpaceId: {type: "string", format: "uuid", nullable: true},
									position: {type: "string", nullable: true},
								},
								required: ["toEntityId"],
							},
						},
						required: ["relationId", "typeId", "spaceId", "changeType"],
					},
					BlockChange: {
						type: "object",
						description: "A block change (text, image, or data block)",
						properties: {
							id: {type: "string", format: "uuid"},
							type: {
								type: "string",
								enum: ["textBlock", "imageBlock", "dataBlock"],
							},
							before: {type: "string", nullable: true},
							after: {type: "string", nullable: true},
							diff: {
								type: "array",
								items: {$ref: "#/components/schemas/DiffChunk"},
								description: "Only present for textBlock type",
							},
						},
						required: ["id", "type"],
					},
					GroupedEntityDiffResponse: {
						type: "object",
						description:
							"A grouped entity diff response. Dynamic group keys from the 'groups' object are spread at the root level.",
						properties: {
							entityId: {type: "string", format: "uuid"},
							name: {type: "string", nullable: true},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/ValueChange"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/RelationChange"},
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockChange"},
								description: "Static key for BLOCKS relation type changes",
							},
							groupKeys: {
								type: "array",
								items: {type: "string"},
								description: "Dynamic group keys present (excluding 'blocks')",
							},
						},
						additionalProperties: {
							type: "array",
							items: {$ref: "#/components/schemas/BlockChange"},
							description: "Dynamic groups by relation type ID",
						},
						required: ["entityId", "values", "relations", "blocks", "groupKeys"],
					},
					// Proposal status types
					ProposalStatusResponse: {
						type: "object",
						description: "Computed proposal status with vote counts and timing info",
						properties: {
							proposalId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							name: {
								type: "string",
								nullable: true,
								description: "Human-readable proposal name",
							},
							status: {
								type: "string",
								enum: ["PROPOSED", "EXECUTABLE", "ACCEPTED", "REJECTED"],
								description: "Current status of the proposal",
							},
							votingMode: {
								type: "string",
								enum: ["FAST", "SLOW"],
								description: "Voting mode determines threshold calculation",
							},
							votes: {
								type: "object",
								properties: {
									yes: {type: "integer", minimum: 0},
									no: {type: "integer", minimum: 0},
									abstain: {type: "integer", minimum: 0},
									total: {type: "integer", minimum: 0},
								},
								required: ["yes", "no", "abstain", "total"],
							},
							quorum: {
								type: "object",
								description: "Quorum progress information",
								properties: {
									required: {
										type: "integer",
										description: "Required votes for quorum",
									},
									current: {
										type: "integer",
										description: "Current total votes",
									},
									progress: {
										type: "number",
										description: "Progress as decimal (0.0 to 1.0)",
									},
									reached: {type: "boolean"},
								},
								required: ["required", "current", "progress", "reached"],
							},
							threshold: {
								type: "object",
								description: "Threshold progress information",
								properties: {
									required: {
										type: "string",
										description: "Required threshold (bigint as string)",
									},
									current: {
										type: "integer",
										description: "Current yes votes",
									},
									progress: {
										type: "number",
										description: "Progress as decimal (0.0 to 1.0)",
									},
									reached: {type: "boolean"},
								},
								required: ["required", "current", "progress", "reached"],
							},
							timing: {
								type: "object",
								properties: {
									startTime: {
										type: "integer",
										description: "Unix timestamp when voting starts",
									},
									endTime: {
										type: "integer",
										description: "Unix timestamp when voting ends",
									},
									timeRemaining: {
										type: "integer",
										nullable: true,
										description: "Seconds until voting ends, null if ended",
									},
									isVotingEnded: {type: "boolean"},
								},
								required: ["startTime", "endTime", "timeRemaining", "isVotingEnded"],
							},
							canExecute: {
								type: "boolean",
								description: "True if proposal can be executed on-chain",
							},
						},
						required: [
							"proposalId",
							"spaceId",
							"name",
							"status",
							"votingMode",
							"votes",
							"quorum",
							"threshold",
							"timing",
							"canExecute",
						],
					},
					ProposalListResponse: {
						type: "object",
						description: "Paginated list of proposal statuses",
						properties: {
							proposals: {
								type: "array",
								items: {$ref: "#/components/schemas/ProposalStatusResponse"},
							},
							nextCursor: {
								type: "string",
								nullable: true,
								description: "Cursor for next page, null if no more results",
							},
						},
						required: ["proposals", "nextCursor"],
					},
					// Proposal diff types
					EntityDiff: {
						type: "object",
						description: "A diff between two versions of an entity",
						properties: {
							entityId: {type: "string", format: "uuid"},
							name: {type: "string", nullable: true},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/ValueChange"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/RelationChange"},
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockChange"},
							},
						},
						required: ["entityId", "values", "relations", "blocks"],
					},
					PaginatedProposalDiff: {
						type: "object",
						description:
							"Paginated response for proposal diffs. Compares proposed changes against base state.",
						properties: {
							proposalId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							proposalStatus: {
								type: "string",
								enum: ["active", "closed", "executed"],
								description:
									"Proposal status: active (compare vs live), closed/executed (compare vs versioned at end_time)",
							},
							entities: {
								type: "array",
								items: {$ref: "#/components/schemas/EntityDiff"},
								description: "Entity diffs for this page",
							},
							pagination: {
								type: "object",
								properties: {
									cursor: {
										type: "string",
										nullable: true,
										description: "Base64-encoded cursor for next page",
									},
									hasMore: {type: "boolean"},
									totalEntities: {
										type: "integer",
										description: "Total number of affected entities",
									},
								},
								required: ["cursor", "hasMore", "totalEntities"],
							},
						},
						required: ["proposalId", "spaceId", "proposalStatus", "entities", "pagination"],
					},
				},
			},
		},
	}),
)

export default app
