import {Duration, Effect, Schedule} from "effect"
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
	const run = Effect.gen(function* () {
		const config = yield* Environment

		const blob = new Blob([file], {type: "application/octet-stream"})
		const formData = new FormData()
		formData.append("file", blob)

		yield* Effect.logInfo("[IPFS][binary] Uploading content...")
		const hash = yield* upload(formData, config.ipfsGatewayWrite).pipe(Effect.withSpan("ipfs.uploadEdit.upload"))
		yield* Effect.logInfo("[IPFS][binary] Validating CID")
		yield* validateCid(hash)
		yield* Effect.logInfo("[IPFS][binary] Uploaded to IPFS successfully")

		return {
			cid: hash as `ipfs://${string}`,
		}
	})

	return Effect.retry(run, {
		schedule: Schedule.exponential("100 millis").pipe(
			Schedule.jittered,
			Schedule.compose(Schedule.elapsed),
			Schedule.tapInput(() => Effect.succeed(console.log("[IPFS][upload] Retrying"))),
			Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(30))),
		),
	})
}

export function uploadFile(file: File) {
	const run = Effect.gen(function* () {
		const config = yield* Environment

		const formData = new FormData()
		formData.append("file", file)

		yield* Effect.logInfo("[IPFS][upload] Uploading content...")
		const hash = yield* upload(formData, config.ipfsGatewayWrite).pipe(Effect.withSpan("ipfs.uploadFile.upload"))
		yield* Effect.logInfo("[IPFS][upload] Uploaded to IPFS successfully")

		return {
			cid: hash,
		}
	})

	return Effect.retry(run, {
		schedule: Schedule.exponential("100 millis").pipe(
			Schedule.jittered,
			Schedule.compose(Schedule.elapsed),
			Schedule.tapInput(() => Effect.succeed(console.log("[IPFS][upload] Retrying"))),
			Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(30))),
		),
	})
}

class IpfsUploadError extends Error {
	readonly _tag = "IpfsUploadError"
}

class IpfsParseResponseError extends Error {
	readonly _tag = "IpfsParseResponseError"
}

export function upload(formData: FormData, url: string) {
	return Effect.gen(function* () {
		yield* Effect.logInfo("[IPFS] Posting IPFS content")
		const config = yield* Environment

		const response = yield* Effect.tryPromise({
			try: () =>
				fetch(url, {
					method: "POST",
					body: formData,
					headers: {
						Authorization: `Bearer ${config.ipfsKey}`,
					},
				}),
			catch: (error) => new IpfsUploadError(`IPFS upload failed: ${error}`),
		}).pipe(Effect.withSpan("ipfs.upload"))

		const {Hash} = yield* Effect.tryPromise({
			try: () => response.json(),
			catch: (error) => new IpfsParseResponseError(`Could not parse IPFS JSON response: ${error}`),
		})

		return `ipfs://${Hash}` as const
	})
}
