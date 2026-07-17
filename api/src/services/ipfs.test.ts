import {Effect} from "effect"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {Environment, type IEnvironment} from "./environment"
import {upload} from "./ipfs"

// =============================================================================
// Helpers
// =============================================================================

const TEST_ENV: IEnvironment = {
	databaseUrl: "postgresql://test" as unknown as IEnvironment["databaseUrl"],
	debug: null,
	ipfsKey: "test-key",
	ipfsGatewayWrite: "https://rpc.filebase.io/api/v0/add",
}

function runUpload(formData: FormData, url = TEST_ENV.ipfsGatewayWrite) {
	return Effect.runPromise(upload(formData, url).pipe(Effect.provideService(Environment, TEST_ENV)))
}

function jsonLines(...entries: Record<string, unknown>[]) {
	return entries.map((e) => JSON.stringify(e)).join("\n")
}

function mockFetchOnce(body: string, init?: {status?: number; statusText?: string}) {
	const response = new Response(body, {status: init?.status ?? 200, statusText: init?.statusText ?? "OK"})
	vi.stubGlobal(
		"fetch",
		vi.fn(() => Promise.resolve(response)),
	)
}

beforeEach(() => {
	vi.stubGlobal("fetch", vi.fn())
})

afterEach(() => {
	vi.unstubAllGlobals()
})

// =============================================================================
// Tests — Filebase (Kubo-compatible /api/v0/add) response parsing (GEO-2323)
// =============================================================================

describe("upload", () => {
	it("extracts the CID from a single-line NDJSON response", async () => {
		mockFetchOnce(jsonLines({Name: "file.bin", Hash: "bafkreitestcid123", Size: "11"}))

		const cid = await runUpload(new FormData())

		expect(cid).toBe("ipfs://bafkreitestcid123")
	})

	it("takes the last Hash when the gateway streams multiple NDJSON lines", async () => {
		mockFetchOnce(
			jsonLines(
				{Name: "dir", Hash: "bafkreidirhash", Size: "0"},
				{Name: "dir/file.bin", Hash: "bafkreifilehash", Size: "11"},
			),
		)

		const cid = await runUpload(new FormData())

		expect(cid).toBe("ipfs://bafkreifilehash")
	})

	it("ignores blank lines between NDJSON entries", async () => {
		mockFetchOnce(`${JSON.stringify({Hash: "bafkreitestcid123"})}\n\n`)

		const cid = await runUpload(new FormData())

		expect(cid).toBe("ipfs://bafkreitestcid123")
	})

	it("fails when the gateway returns a non-2xx status", async () => {
		mockFetchOnce("Internal Server Error", {status: 500, statusText: "Internal Server Error"})

		await expect(runUpload(new FormData())).rejects.toThrow()
	})

	it("fails when a streamed line reports a gateway error", async () => {
		mockFetchOnce(jsonLines({Type: "error", Message: "context deadline exceeded"}))

		await expect(runUpload(new FormData())).rejects.toThrow()
	})

	it("fails when the response body is not JSON at all", async () => {
		mockFetchOnce("<html>not json</html>")

		await expect(runUpload(new FormData())).rejects.toThrow()
	})

	it("fails when the response has no Hash anywhere", async () => {
		mockFetchOnce(jsonLines({Name: "file.bin", Size: "11"}))

		await expect(runUpload(new FormData())).rejects.toThrow()
	})
})
