import {spawn as nodeSpawn} from "node:child_process"
import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {readGatewayConfig} from "../../gateway/config"
import {createGateway} from "../../gateway/gateway"
import {
	__resetGatewayMetricsForTests,
	activateGatewayMetrics,
	recordGatewayShadow,
	renderGatewayMetrics,
} from "../../gateway/metrics"
import {
	__resetProfileShadowMetricsForTests,
	createProfileShadow,
	readProfileShadowConfig,
	renderProfileShadowMetrics,
} from "../../profile/shadow"
import {createPipeGeoDbClient, GeoDbError} from "../client"
import type {GeoDbCardConfig} from "../config"
import {BACKING_OFF_MESSAGE, FAIL_REASONS, failReason, notedFailReason, withFailureScope} from "../failReason"
import {__resetGeoDbMetricsForTests, activateGeoDbMetrics, renderGeoDbCardMetrics} from "../metrics"
import {type CardDeps, resolveCard} from "../service"

const ENTITY = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "22222222222242228222222222222222"
const PROFILE_ENTITY = "11111111111141118111111111111111"

/** A fake `geodb-client pipe` run through the client's spawn seam. */
function fakePipe(body: string, timeoutMs = 2000) {
	const dir = mkdtempSync(join(tmpdir(), "geodb-failreason-"))
	const script = join(dir, "fake.mjs")
	writeFileSync(
		script,
		`const frame = (kind, text) => process.stdout.write(kind + Buffer.byteLength(text) + "\\n" + text)
const lines = (onLine) => {
	let buffer = ""
	process.stdin.setEncoding("utf8")
	process.stdin.on("data", (chunk) => {
		buffer += chunk
		for (let i; (i = buffer.indexOf("\\n")) >= 0; ) {
			const line = buffer.slice(0, i)
			buffer = buffer.slice(i + 1)
			onLine(line)
		}
	})
}
${body}
`,
	)
	const config: GeoDbCardConfig = {
		mode: "on",
		clientBin: process.execPath,
		keyPath: "k",
		bindingPath: "b",
		spaceMapPath: "",
		timeoutMs,
		poolSize: 1,
	}
	return createPipeGeoDbClient(config, {
		spawn: () => nodeSpawn(process.execPath, [script], {stdio: ["pipe", "pipe", "pipe"]}) as never,
	})
}

const NEVER_ANSWERS = `process.stdout.write("ready\\n")\nlines(() => {})`
const REFUSES = `process.stdout.write("ready\\n")\nlines(() => { frame("-", "refused: x"); process.exit(1) })`
const NEVER_READY = `console.error("connect: ConnectionRefused")\nprocess.exit(1)`

const read = (client: ReturnType<typeof fakePipe>) => client.readCard({geoSpaceId: SPACE, entityId: ENTITY})

/** What the failure scope noted after one read through the pool, which is what a gateway shadow's `failed` counts. */
async function notedAfter(client: ReturnType<typeof fakePipe>, reads = 1) {
	return withFailureScope(async () => {
		for (let i = 0; i < reads; i++) await read(client).catch(() => {})
		return notedFailReason()
	})
}

describe("failReason", () => {
	it("keeps the label set fixed", () => {
		expect([...FAIL_REASONS]).toEqual(["timeout", "client_failed", "cooling_down", "other"])
	})

	it("classifies one error per reason, and every unknown error as other", () => {
		expect(failReason(new GeoDbError("timeout", "slow"))).toBe("timeout")
		expect(failReason(new GeoDbError("client_failed", "worker died"))).toBe("client_failed")
		expect(failReason(new GeoDbError("client_failed", BACKING_OFF_MESSAGE))).toBe("cooling_down")
		expect(failReason(new GeoDbError("overloaded", "queue full"))).toBe("other")
		expect(failReason(new GeoDbError("bad_request", "ids"))).toBe("other")
		expect(failReason(new Error("anything at all, never a label"))).toBe("other")
		expect(failReason("text")).toBe("other")
	})

	it("is other outside a scope, and the first failure wins inside one", async () => {
		expect(notedFailReason()).toBe("other")
		const client = fakePipe(NEVER_ANSWERS, 300)
		expect(await notedAfter(client)).toBe("timeout")
		client.close()
	})
})

describe("the pool notes why a read failed", () => {
	it("timeout", async () => {
		const client = fakePipe(NEVER_ANSWERS, 300)
		expect(await notedAfter(client)).toBe("timeout")
		client.close()
	})

	it("client_failed", async () => {
		const client = fakePipe(REFUSES)
		expect(await notedAfter(client)).toBe("client_failed")
		client.close()
	})

	it("cooling_down: the second read lands in the backoff after a worker that never started", async () => {
		const client = fakePipe(NEVER_READY)
		await read(client).catch(() => {})
		expect(await notedAfter(client)).toBe("cooling_down")
		client.close()
	})

	it("other: a read the pool refuses as a bad request", async () => {
		const client = fakePipe(NEVER_ANSWERS)
		const noted = await withFailureScope(async () => {
			await client.readCard({geoSpaceId: SPACE, entityId: "nope"}).catch(() => {})
			return notedFailReason()
		})
		expect(noted).toBe("other")
		client.close()
	})
})

describe("gaia_api_geodb_card_shadow_failed_total", () => {
	beforeEach(() => __resetGeoDbMetricsForTests())
	const GEO_SPACE = "22222222222242228222222222222222"
	const deps = (error: unknown): CardDeps => ({
		mode: "shadow",
		geoDb: {
			readCard: async () => {
				throw error
			},
		},
		geoSpaceOf: async () => GEO_SPACE,
		readPostgres: async () => ({values: [], relations: []}),
		log: {info: vi.fn(), warn: vi.fn()},
	})
	const series = (reason: string) =>
		renderGeoDbCardMetrics()
			.split("\n")
			.find((l) => l.startsWith(`gaia_api_geodb_card_shadow_failed_total{reason="${reason}"} `))
			?.split(" ")
			.pop()

	it("counts each reason once and keeps the unlabelled failed series as their sum", async () => {
		activateGeoDbMetrics()
		for (const error of [
			new GeoDbError("timeout", "slow"),
			new GeoDbError("client_failed", "died"),
			new GeoDbError("client_failed", BACKING_OFF_MESSAGE),
			new Error("odd"),
		])
			await (await resolveCard(deps(error), ENTITY, "11111111111141118111111111111111")).shadow
		for (const reason of FAIL_REASONS) expect(series(reason)).toBe("1")
		expect(renderGeoDbCardMetrics()).toContain('gaia_api_geodb_card_shadow_total{result="failed"} 4')
	})
})

describe("gaia_api_profile_shadow_failed_total", () => {
	beforeEach(() => __resetProfileShadowMetricsForTests())
	const settle = () => new Promise((r) => setTimeout(r, 0))
	const count = (surface: string, reason: string) =>
		Number(
			renderProfileShadowMetrics().match(
				new RegExp(`gaia_api_profile_shadow_failed_total\\{surface="${surface}",reason="${reason}"\\} (\\d+)`),
			)?.[1],
		)

	it("counts each reason, and the unlabelled failed series still counts them all", async () => {
		const errors = [
			new GeoDbError("timeout", "slow"),
			new GeoDbError("client_failed", "died"),
			new GeoDbError("client_failed", BACKING_OFF_MESSAGE),
			new Error("odd"),
		]
		let next = 0
		const shadow = createProfileShadow({
			config: readProfileShadowConfig({GEODB_PROFILE_SHADOW: "on"}),
			geoDb: {
				readCard: async () => {
					throw errors[next++]
				},
			} as never,
			geoSpaceOf: async () => "s",
			log: {warn: vi.fn()},
		})
		for (let i = 0; i < errors.length; i++) {
			shadow.profiles("address", [
				{entityId: PROFILE_ENTITY, spaceId: SPACE, name: "Ada", avatarUrl: null, address: "0xab"} as never,
			])
			await settle()
			await settle()
		}
		for (const reason of FAIL_REASONS) expect(count("address", reason)).toBe(1)
		expect(renderProfileShadowMetrics()).toContain(
			'gaia_api_profile_shadow_total{surface="address",result="failed"} 4',
		)
	})
})

describe("gaia_api_gateway_shadow_failed_total", () => {
	beforeEach(() => __resetGatewayMetricsForTests())
	const wait = () => new Promise((resolve) => setTimeout(resolve, 20))
	const post = () =>
		new Request("http://x/graphql", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({query: "{ metas { nodes { id } } }"}),
		})
	const delegate = async () =>
		new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}})
	const series = (reason: string) =>
		renderGatewayMetrics()
			.split("\n")
			.find((l) => l.startsWith(`gaia_api_gateway_shadow_failed_total{field="metas",reason="${reason}"} `))
			?.split(" ")
			.pop()

	/** A shadow as the real ones fail: a pool read rejects somewhere down, and the shadow only counts `failed`. */
	async function runShadowOver(client: ReturnType<typeof fakePipe>) {
		activateGatewayMetrics("metas")
		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "metas=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
			}),
			delegate,
			shadows: new Map([
				[
					"metas",
					async () => {
						await read(client).catch(() => {})
						recordGatewayShadow("metas", "failed")
					},
				],
			]),
			random: () => 0,
		})
		await gateway(post(), {})
		await new Promise((resolve) => setTimeout(resolve, 600))
	}

	it("timeout", async () => {
		const client = fakePipe(NEVER_ANSWERS, 200)
		await runShadowOver(client)
		expect(series("timeout")).toBe("1")
		client.close()
	})

	it("client_failed", async () => {
		const client = fakePipe(REFUSES)
		await runShadowOver(client)
		expect(series("client_failed")).toBe("1")
		client.close()
	})

	it("cooling_down", async () => {
		const client = fakePipe(NEVER_READY)
		await read(client).catch(() => {})
		await runShadowOver(client)
		// A read the pool never tried is not a failed shadow: it counts unsupported_selection / cooling_down instead.
		expect(series("cooling_down")).toBe("0")
		expect(renderGatewayMetrics()).toContain(
			'gaia_api_gateway_shadow_unsupported_total{field="metas",reason="cooling_down"} 1',
		)
		client.close()
	})

	it("other: a failure with no pool read behind it", async () => {
		activateGatewayMetrics("metas")
		recordGatewayShadow("metas", "failed")
		await wait()
		expect(series("other")).toBe("1")
		expect(renderGatewayMetrics()).toContain('gaia_api_gateway_shadow_total{field="metas",result="failed"} 1')
	})
})
