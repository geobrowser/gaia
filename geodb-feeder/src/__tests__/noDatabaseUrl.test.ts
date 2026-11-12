/**
 * The feeder Deployment is meant to run with no DATABASE_URL (zero-Postgres plan 22 section 20): Postgres is needed only
 * by the membership shadow compare, which fails closed, and by the backfill and replay entrypoints.
 */
import {readdirSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {afterEach, describe, expect, it, vi} from "vitest"
import {feederGroupIds, loadConfig} from "../config.js"
import {shadowComparePg} from "../shadowComparePg.js"

const src = join(dirname(fileURLToPath(import.meta.url)), "..")

// Every consumer and publisher flag on, and none of DATABASE_URL / DATABASE_SSL_CA.
const allOn = {
	KAFKA_BROKER: "b",
	KAFKA_USERNAME: "u",
	KAFKA_PASSWORD: "p",
	KAFKA_SSL_CA_PEM: "c",
	GEODB_KEY_PATH: "k",
	GEODB_BINDING_PATH: "b",
	GEODB_APPLIED_BLOCK_KEY_PATH: "a",
	GEODB_MEMBERSHIP_DUAL_WRITE: "1",
	GEODB_MEMBERSHIP_SHADOW_COMPARE: "1",
	GEODB_RECORD_KEYS: "1",
	GEODB_PROPOSAL_ENTITIES: "1",
	GEODB_SPACE_ENTITIES: "1",
	GEODB_VOTES: "1",
	GEODB_PROPOSAL_VOTES: "1",
	GEODB_PROPOSAL_ACTIONS: "1",
	GEODB_SUBSPACES: "1",
	GEODB_SPACE_VOTING_SETTINGS: "1",
	GEODB_SPACE_TOPICS: "1",
	GEODB_SPACE_ADDRESSES: "1",
	GEODB_PROPOSAL_VERSIONS: "1",
	GEODB_PROPOSAL_STATUS: "1",
	GEODB_RANKS: "1",
	GEODB_RELATION_POSITIONS: "1",
	GEODB_ENTITY_TIMES: "1",
	GEODB_LEGACY_ID_MAP: "1",
	GEODB_APPLIED_BLOCK: "1",
	GEODB_FOR_YOU_CONFIG: "1",
	GEODB_ENTITY_RANKING_CONFIG: "1",
	GEODB_INTEREST_CONFIG: "1",
	GEODB_FEED_EXPERIMENTS: "1",
	GEODB_FEED_EXPERIMENT_MEMBERS: "1",
	GEODB_ENTITY_FEED_CONFIG: "1",
}

afterEach(() => vi.restoreAllMocks())

describe("feeder without DATABASE_URL", () => {
	it("loads its config with every flag on", () => {
		expect("DATABASE_URL" in allOn).toBe(false)
		const c = loadConfig(allOn)
		expect(c.membership.dualWrite).toBe(true)
		expect(c.membership.shadowCompare).toBe(true)
		expect(c.recordKeys).toBe(true)
		expect(c.ranks.enabled).toBe(true)
		expect(c.appliedBlock.enabled).toBe(true)
		expect(feederGroupIds(c).length).toBeGreaterThan(10)
	})

	it("the shadow compare fails closed: an error log, no Postgres handle", () => {
		const out = vi.spyOn(process.stdout, "write").mockReturnValue(true)
		expect(shadowComparePg({})).toBeNull()
		expect(shadowComparePg({DATABASE_URL: "", DATABASE_SSL_CA: "ca"})).toBeNull()
		const lines = out.mock.calls.map((c) => JSON.parse(String(c[0])))
		expect(lines).toHaveLength(2)
		expect(lines[0]).toMatchObject({level: "error", msg: expect.stringContaining("GEODB_MEMBERSHIP_SHADOW_COMPARE")})
	})

	it("the shadow compare still opens a handle when DATABASE_URL is set (nothing connects until a read)", () => {
		expect(shadowComparePg({DATABASE_URL: "postgres://u:p@h:5432/d"})).not.toBeNull()
	})

	it("only the compare reads DATABASE_URL on the feeder's own path; the rest is backfill and replay entrypoints", () => {
		const readers = readdirSync(src)
			.filter((f) => f.endsWith(".ts"))
			.filter((f) => /env\.DATABASE_URL|\["DATABASE_URL"\]/.test(readFileSync(join(src, f), "utf8")))
			.sort()
		const nonFeeder = (f: string) => f.endsWith("-main.ts") && f !== "main.ts"
		const feederPath = readers.filter((f) => !nonFeeder(f) && f !== "keyedBackfillJob.ts")
		expect(feederPath).toEqual(["shadowComparePg.ts"])
	})
})
