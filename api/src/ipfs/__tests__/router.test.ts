import {Effect} from "effect"
import {Hono} from "hono"
import {describe, expect, it, vi} from "vitest"
import {createIpfsRouter} from "../index"

// =============================================================================
// Helpers
// =============================================================================

const FAKE_CID = "ipfs://bafkreitestfakecidvaluethatisroughlythirtybytesxx"

function createMockRuntime() {
	return {
		runPromise: <A, E>(effect: Effect.Effect<A, E, never>) => Effect.runPromise(effect),
	}
}

function setupApp(opts?: {uploadEdit?: ReturnType<typeof vi.fn>; uploadFile?: ReturnType<typeof vi.fn>}) {
	const uploadEdit = opts?.uploadEdit ?? vi.fn(() => Effect.succeed({cid: FAKE_CID}))
	const uploadFile = opts?.uploadFile ?? vi.fn(() => Effect.succeed({cid: FAKE_CID}))
	const runtime = createMockRuntime()
	const router = createIpfsRouter(uploadEdit as any, uploadFile as any, runtime as any)
	const app = new Hono()
	app.route("/ipfs", router)
	return {app, uploadEdit, uploadFile}
}

function makeFormData(file: File | undefined) {
	const formData = new FormData()
	if (file) {
		formData.append("file", file)
	}
	return formData
}

const ENDPOINTS = ["/ipfs/upload-edit", "/ipfs/upload-file", "/ipfs/upload-file-alternative-gateway"] as const

// =============================================================================
// Tests
// =============================================================================

describe.each(ENDPOINTS)("POST %s", (endpoint) => {
	it("returns 400 when no file is provided", async () => {
		const {app, uploadEdit, uploadFile} = setupApp()

		const res = await app.request(endpoint, {
			method: "POST",
			body: makeFormData(undefined),
		})

		expect(res.status).toBe(400)
		expect(await res.text()).toBe("No file provided")
		expect(uploadEdit).not.toHaveBeenCalled()
		expect(uploadFile).not.toHaveBeenCalled()
	})

	it("returns 400 when the file is empty (size === 0) without calling the upload service", async () => {
		const {app, uploadEdit, uploadFile} = setupApp()

		const emptyFile = new File([], "empty.bin", {type: "application/octet-stream"})
		expect(emptyFile.size).toBe(0)

		const res = await app.request(endpoint, {
			method: "POST",
			body: makeFormData(emptyFile),
		})

		expect(res.status).toBe(400)
		expect(await res.text()).toBe("File cannot be empty")
		// Critical: the upload service MUST NOT be invoked for empty files —
		// that's the regression this guards against (upstream gateway error → Sentry issue).
		expect(uploadEdit).not.toHaveBeenCalled()
		expect(uploadFile).not.toHaveBeenCalled()
	})

	it("returns 400 with no upload-service call when the request body isn't multipart", async () => {
		const {app, uploadEdit, uploadFile} = setupApp()

		// Bare POST: no Content-Type, no body. c.req.formData() throws on this.
		const res = await app.request(endpoint, {method: "POST"})

		expect(res.status).toBe(400)
		expect(await res.text()).toBe("Invalid multipart body")
		expect(uploadEdit).not.toHaveBeenCalled()
		expect(uploadFile).not.toHaveBeenCalled()
	})

	it("returns 400 with no upload-service call for a JSON body (wrong content-type)", async () => {
		const {app, uploadEdit, uploadFile} = setupApp()

		const res = await app.request(endpoint, {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({not: "multipart"}),
		})

		expect(res.status).toBe(400)
		expect(await res.text()).toBe("Invalid multipart body")
		expect(uploadEdit).not.toHaveBeenCalled()
		expect(uploadFile).not.toHaveBeenCalled()
	})

	it("delegates to the upload service when a non-empty file is provided", async () => {
		const {app, uploadEdit, uploadFile} = setupApp()

		const file = new File(["hello world"], "test.bin", {type: "application/octet-stream"})

		const res = await app.request(endpoint, {
			method: "POST",
			body: makeFormData(file),
		})

		expect(res.status).toBe(200)
		expect(await res.json()).toEqual({cid: FAKE_CID})

		// upload-edit hits uploadEdit; the two file routes share uploadFile.
		if (endpoint === "/ipfs/upload-edit") {
			expect(uploadEdit).toHaveBeenCalledTimes(1)
			expect(uploadFile).not.toHaveBeenCalled()
		} else {
			expect(uploadFile).toHaveBeenCalledTimes(1)
			expect(uploadEdit).not.toHaveBeenCalled()
		}
	})
})
