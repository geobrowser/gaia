/**
 * IPFS upload routes.
 *
 * Three endpoints, all multipart `file` form uploads:
 *   - POST /upload-edit                          — binary GRC-20 edits
 *   - POST /upload-file                          — arbitrary user files (images, etc.)
 *   - POST /upload-file-alternative-gateway      — deprecated alias for /upload-file
 *
 * Validates that a file is provided and non-empty before delegating to the
 * Filebase-backed IPFS service (GEO-2323 — migrated off Pinata). Empty/missing
 * files short-circuit with 400 to avoid producing Sentry issues from upstream
 * gateway errors.
 */

import {Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"
import type {Environment} from "../services/environment"
import type {AppRuntime} from "../services/runtime"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

const EMPTY_FILE_LOG_MESSAGE = "Empty file provided"
const EMPTY_FILE_ERROR_MESSAGE = "File cannot be empty"
const INVALID_MULTIPART_ERROR_MESSAGE = "Invalid multipart body"

// OpenAPI response descriptions — kept as constants so the three routes stay
// in lockstep without copy-paste drift.
const RESPONSE_200_DESCRIPTION = "File successfully uploaded to IPFS"
const RESPONSE_400_DESCRIPTION = "Invalid file (missing or empty)"
const RESPONSE_500_DESCRIPTION = "Upload failed"

type UploadFn = (file: File) => Effect.Effect<{cid: string}, Error, Environment>

/**
 * Build a route handler that runs the shared validation pipeline and
 * delegates to the supplied upload function on success.
 */
function createUploadHandler(opts: {spanName: string; upload: UploadFn; runtime: AppRuntime}) {
	const {spanName, upload, runtime} = opts

	return async (c: import("hono").Context<AppEnv>) => {
		const requestId = c.get("requestId") ?? "unknown"

		// Reject non-multipart bodies (bare POST, wrong content-type, malformed
		// boundary, etc.) with 400 instead of letting the parse exception bubble
		// up as 500. Scanners and ad-hoc curls hit this path; we don't want
		// Sentry issues for malformed client input.
		let formData: FormData
		try {
			formData = await c.req.formData()
		} catch {
			return new Response(INVALID_MULTIPART_ERROR_MESSAGE, {status: 400})
		}
		const file = formData.get("file") as File | undefined

		const program = Effect.gen(function* () {
			if (!file) {
				yield* Effect.logWarning("No file provided")
				return yield* Effect.fail({
					_tag: "ValidationError" as const,
					status: 400,
					message: "No file provided",
				})
			}

			if (file.size === 0) {
				yield* Effect.logWarning(EMPTY_FILE_LOG_MESSAGE)
				return yield* Effect.fail({
					_tag: "ValidationError" as const,
					status: 400,
					message: EMPTY_FILE_ERROR_MESSAGE,
				})
			}

			const result = yield* upload(file).pipe(
				Effect.mapError((error) => ({
					_tag: "UploadError" as const,
					status: 500,
					message: error.message,
				})),
			)

			return result
		}).pipe(
			Effect.withSpan(spanName),
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
	}
}

const fileFormSchema = {
	"multipart/form-data": {
		schema: {
			type: "object" as const,
			properties: {
				file: {
					type: "string" as const,
					format: "binary",
					description: "The file to upload",
				},
			},
			required: ["file"],
		},
	},
}

const cidResponseSchema = {
	"application/json": {
		schema: {
			type: "object" as const,
			properties: {
				cid: {
					type: "string" as const,
					description: "The IPFS content identifier (CID) of the uploaded file",
				},
			},
			required: ["cid"],
		},
	},
}

const validationErrorSchema = {
	"text/plain": {
		schema: {
			type: "string" as const,
			example: "No file provided",
		},
	},
}

const uploadFailedSchema = {
	"text/plain": {
		schema: {type: "string" as const},
	},
}

/**
 * Create the IPFS upload router.
 *
 * @param uploadEdit - Effect-based uploader for binary edits (Pinata-backed in production)
 * @param uploadFile - Effect-based uploader for arbitrary files (Pinata-backed in production)
 * @param runtime    - Effect runtime that supplies Environment + telemetry
 */
export function createIpfsRouter(uploadEdit: UploadFn, uploadFile: UploadFn, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	router.post(
		"/upload-edit",
		describeRoute({
			tags: ["IPFS"],
			summary: "Upload an edit to IPFS",
			description: "Uploads an edit file to IPFS and returns the content identifier (CID)",
			requestBody: {content: fileFormSchema},
			responses: {
				200: {description: RESPONSE_200_DESCRIPTION, content: cidResponseSchema},
				400: {description: RESPONSE_400_DESCRIPTION, content: validationErrorSchema},
				500: {description: RESPONSE_500_DESCRIPTION, content: uploadFailedSchema},
			},
		}),
		createUploadHandler({spanName: "/ipfs/upload-edit", upload: uploadEdit, runtime}),
	)

	router.post(
		"/upload-file",
		describeRoute({
			tags: ["IPFS"],
			summary: "Upload a file to IPFS",
			description: "Uploads a file to IPFS and returns the content identifier (CID)",
			requestBody: {content: fileFormSchema},
			responses: {
				200: {description: RESPONSE_200_DESCRIPTION, content: cidResponseSchema},
				400: {description: RESPONSE_400_DESCRIPTION, content: validationErrorSchema},
				500: {description: RESPONSE_500_DESCRIPTION, content: uploadFailedSchema},
			},
		}),
		createUploadHandler({spanName: "/ipfs/upload-file", upload: uploadFile, runtime}),
	)

	// Backwards compatibility alias - same implementation as /upload-file
	router.post(
		"/upload-file-alternative-gateway",
		describeRoute({
			tags: ["IPFS"],
			summary: "Upload a file to IPFS (deprecated)",
			description:
				"Deprecated: Use /ipfs/upload-file instead. This endpoint is maintained for backwards compatibility.",
			deprecated: true,
			requestBody: {content: fileFormSchema},
			responses: {
				200: {description: RESPONSE_200_DESCRIPTION, content: cidResponseSchema},
				400: {description: RESPONSE_400_DESCRIPTION, content: validationErrorSchema},
				500: {description: RESPONSE_500_DESCRIPTION, content: uploadFailedSchema},
			},
		}),
		createUploadHandler({
			spanName: "/ipfs/upload-file-alternative-gateway",
			upload: uploadFile,
			runtime,
		}),
	)

	return router
}
