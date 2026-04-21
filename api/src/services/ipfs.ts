import {Duration, Effect, Ref, Schedule} from "effect"
import {Environment} from "./environment"

class CidValidateError extends Error {
	readonly _tag = "CidValidateError"
}

function validateCid(cid: string) {
	return Effect.gen(function* () {
		const [, cidContains] = cid.split("ipfs://")
		if (!cid.startsWith("ipfs://")) {
			yield* Effect.fail(new CidValidateError(`CID ${cid} does not start with ipfs://`))
		}

		if (cidContains === undefined || cidContains === "") {
			yield* Effect.fail(new CidValidateError(`CID ${cid} is not valid`))
		}

		return true
	})
}

export function uploadEdit(file: File) {
	return Effect.gen(function* () {
		const config = yield* Environment
		const attemptRef = yield* Ref.make(0)
		const startTime = Date.now()

		const run = Effect.gen(function* () {
			yield* Ref.update(attemptRef, (n) => n + 1)

			const blob = new Blob([file], {type: "application/octet-stream"})
			const formData = new FormData()
			formData.append("network", "public")
			formData.append("file", blob, file.name || "edit.bin")

			const hash = yield* upload(formData, config.ipfsGatewayWrite)
			yield* validateCid(hash)

			return hash as `ipfs://${string}`
		})

		const cid = yield* Effect.retry(run, {
			schedule: Schedule.exponential("100 millis").pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(30))),
			),
		}).pipe(
			Effect.tapError((error) =>
				Effect.gen(function* () {
					const attempts = yield* Ref.get(attemptRef)
					yield* Effect.logError("[IPFS] uploadEdit failed", {
						cause: error,
						fileName: file.name,
						fileSize: file.size,
						attempts,
						durationMs: Date.now() - startTime,
					})
				}),
			),
		)

		const attempts = yield* Ref.get(attemptRef)

		// Canonical end log with full context
		yield* Effect.logInfo("[IPFS] uploadEdit completed", {
			fileName: file.name,
			fileSize: file.size,
			cid,
			attempts,
			durationMs: Date.now() - startTime,
		})

		return {cid}
	}).pipe(Effect.withSpan("ipfs.uploadEdit"))
}

export function uploadFile(file: File) {
	return Effect.gen(function* () {
		const config = yield* Environment
		const attemptRef = yield* Ref.make(0)
		const startTime = Date.now()

		const run = Effect.gen(function* () {
			yield* Ref.update(attemptRef, (n) => n + 1)

			const formData = new FormData()
			formData.append("network", "public")
			// Always provide filename - Bun hangs indefinitely without it
			formData.append("file", file, file.name || "file.bin")

			return yield* upload(formData, config.ipfsGatewayWrite)
		})

		const cid = yield* Effect.retry(run, {
			schedule: Schedule.exponential("100 millis").pipe(
				Schedule.jittered,
				Schedule.compose(Schedule.elapsed),
				Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(30))),
			),
		}).pipe(
			Effect.tapError((error) =>
				Effect.gen(function* () {
					const attempts = yield* Ref.get(attemptRef)
					yield* Effect.logError("[IPFS] uploadFile failed", {
						cause: error,
						fileName: file.name,
						fileSize: file.size,
						attempts,
						durationMs: Date.now() - startTime,
					})
				}),
			),
		)

		const attempts = yield* Ref.get(attemptRef)

		// Canonical end log with full context
		yield* Effect.logInfo("[IPFS] uploadFile completed", {
			fileName: file.name,
			fileSize: file.size,
			cid,
			attempts,
			durationMs: Date.now() - startTime,
		})

		return {cid}
	}).pipe(Effect.withSpan("ipfs.uploadFile"))
}

class IpfsUploadError extends Error {
	readonly _tag = "IpfsUploadError"
}

class IpfsParseResponseError extends Error {
	readonly _tag = "IpfsParseResponseError"
}

export function upload(formData: FormData, url: string) {
	return Effect.gen(function* () {
		const config = yield* Environment
		const requestStart = Date.now()

		const response = yield* Effect.tryPromise({
			try: () =>
				fetch(url, {
					method: "POST",
					body: formData,
					headers: {
						Authorization: `Bearer ${config.ipfsKey}`,
					},
				}),
			catch: (error) => new IpfsUploadError(`IPFS fetch failed: ${error}`),
		})

		const responseText = yield* Effect.tryPromise({
			try: () => response.text(),
			catch: (error) => new IpfsParseResponseError(`Could not read IPFS response body: ${error}`),
		})

		const diagnostics = {
			url,
			httpStatus: response.status,
			httpStatusText: response.statusText,
			responseTimeMs: Date.now() - requestStart,
			contentType: response.headers.get("content-type"),
			contentLength: response.headers.get("content-length"),
			pinataRequestId: response.headers.get("x-pinata-request-id") ?? response.headers.get("x-request-id"),
			cfRay: response.headers.get("cf-ray"),
			server: response.headers.get("server"),
			retryAfter: response.headers.get("retry-after"),
			rateLimitLimit: response.headers.get("x-ratelimit-limit"),
			rateLimitRemaining: response.headers.get("x-ratelimit-remaining"),
			rateLimitReset: response.headers.get("x-ratelimit-reset"),
			bodyLength: responseText.length,
			bodyPreview: responseText.slice(0, 1000),
		}

		if (!response.ok) {
			yield* Effect.logWarning("[IPFS] gateway returned non-2xx status", diagnostics)
			yield* Effect.fail(new IpfsUploadError(`IPFS gateway HTTP ${response.status} ${response.statusText}`))
		}

		const responseJson = yield* Effect.try({
			try: () => JSON.parse(responseText) as {error?: unknown; data?: {cid?: string}},
			catch: () => new IpfsParseResponseError(`Could not parse IPFS JSON response (status=${response.status})`),
		}).pipe(Effect.tapError(() => Effect.logWarning("[IPFS] gateway returned non-JSON response", diagnostics)))

		// Handle error responses from gateway
		if (responseJson.error) {
			const errorMsg =
				typeof responseJson.error === "object" && responseJson.error !== null
					? (responseJson.error as {message?: string}).message || JSON.stringify(responseJson.error)
					: String(responseJson.error)
			yield* Effect.logWarning("[IPFS] gateway returned error in body", {
				...diagnostics,
				gatewayErrorMessage: errorMsg,
			})
			yield* Effect.fail(new IpfsUploadError(`IPFS gateway error: ${errorMsg}`))
		}

		const cid = responseJson.data?.cid
		if (!cid) {
			yield* Effect.logWarning("[IPFS] gateway returned no CID", diagnostics)
			return yield* Effect.fail(new IpfsUploadError("IPFS gateway returned no CID"))
		}

		return `ipfs://${cid}` as const
	}).pipe(Effect.withSpan("ipfs.upload"))
}
