import {describe, expect, test} from "bun:test"

import {ConfigError, parseConfig} from "../src/config.js"

/** A complete, valid environment. Individual tests clone and mutate it. */
function validEnv(): NodeJS.ProcessEnv {
	return {
		GEO_WEBHOOK_SECRET: "s3cr3t",
		ACCEPTOR_PRIVATE_KEY: `0x${"1".repeat(64)}`,
		ACCEPTOR_SPACE_ID: `0x${"a".repeat(32)}`,
		SPACE_REGISTRY_ADDRESS: `0x${"b".repeat(40)}`,
		RPC_URL: "https://rpc.example",
		PIMLICO_API_KEY: "pim_test",
		GRAPHQL_ENDPOINT: "https://api.example/graphql",
		MEMBERSHIP_AUTOACCEPT_SPACE_IDS: "d4f5a6b7-0000-0000-0000-000000000000",
	}
}

describe("parseConfig", () => {
	test("parses a full valid environment", () => {
		const c = parseConfig(validEnv())
		expect(c.port).toBe(8080)
		expect(c.webhookSecret).toBe("s3cr3t")
		expect(c.chainId).toBe(80451) // mainnet default
		expect(c.spaceRegistryAddress).toBe(`0x${"b".repeat(40)}`)
		// Allowlist is canonicalized to 0x bytes16 (the dashed UUID's dashless form).
		expect([...c.autoacceptSpaceIds]).toEqual(["0xd4f5a6b7000000000000000000000000"])
	})

	test("adds the 0x prefix to a bare private key", () => {
		const c = parseConfig({...validEnv(), ACCEPTOR_PRIVATE_KEY: "1".repeat(64)})
		expect(c.acceptorPrivateKey).toBe(`0x${"1".repeat(64)}`)
	})

	test("canonicalizes, de-duplicates, and drops blanks in the allowlist", () => {
		// Same space in 0x-upper, dashless-lower, and dashed-UUID forms → one entry;
		// plus a distinct space. All canonicalize to 0x bytes16.
		const dupA0x = `0x${"A".repeat(32)}`
		const dupADashless = "a".repeat(32)
		const distinctUuid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
		const c = parseConfig({
			...validEnv(),
			MEMBERSHIP_AUTOACCEPT_SPACE_IDS: ` ${dupA0x} , ${dupADashless} ,, ${distinctUuid} `,
		})
		expect([...c.autoacceptSpaceIds].sort()).toEqual([`0x${"a".repeat(32)}`, `0x${"b".repeat(32)}`])
	})

	test("throws on an allowlist entry that isn't a valid space id", () => {
		expect(() => parseConfig({...validEnv(), MEMBERSHIP_AUTOACCEPT_SPACE_IDS: "not-a-real-id"})).toThrow(
			ConfigError,
		)
	})

	test("an empty allowlist is allowed (kill switch)", () => {
		const c = parseConfig({...validEnv(), MEMBERSHIP_AUTOACCEPT_SPACE_IDS: ""})
		expect(c.autoacceptSpaceIds.size).toBe(0)
	})

	test("honors a valid PORT and CHAIN_ID", () => {
		const c = parseConfig({...validEnv(), PORT: "3000", CHAIN_ID: "19411"})
		expect(c.port).toBe(3000)
		expect(c.chainId).toBe(19411)
	})

	test.each([
		["GEO_WEBHOOK_SECRET", {GEO_WEBHOOK_SECRET: ""}],
		["ACCEPTOR_PRIVATE_KEY", {ACCEPTOR_PRIVATE_KEY: undefined}],
		["ACCEPTOR_SPACE_ID", {ACCEPTOR_SPACE_ID: undefined}],
		["SPACE_REGISTRY_ADDRESS", {SPACE_REGISTRY_ADDRESS: undefined}],
		["RPC_URL", {RPC_URL: undefined}],
		["PIMLICO_API_KEY", {PIMLICO_API_KEY: undefined}],
		["GRAPHQL_ENDPOINT", {GRAPHQL_ENDPOINT: undefined}],
	])("throws when %s is missing", (_label, override) => {
		expect(() => parseConfig({...validEnv(), ...override})).toThrow(ConfigError)
	})

	test("throws on a malformed private key", () => {
		expect(() => parseConfig({...validEnv(), ACCEPTOR_PRIVATE_KEY: "0xnothex"})).toThrow(ConfigError)
	})

	test("throws on a malformed ACCEPTOR_SPACE_ID (not bytes16)", () => {
		expect(() => parseConfig({...validEnv(), ACCEPTOR_SPACE_ID: `0x${"a".repeat(40)}`})).toThrow(ConfigError)
	})

	test("throws on a malformed SPACE_REGISTRY_ADDRESS", () => {
		expect(() => parseConfig({...validEnv(), SPACE_REGISTRY_ADDRESS: "0x1234"})).toThrow(ConfigError)
	})

	test("throws on an unsupported CHAIN_ID", () => {
		expect(() => parseConfig({...validEnv(), CHAIN_ID: "1"})).toThrow(ConfigError)
	})

	// GEO-2478: 55516 was rejected outright, so the acceptor could not run on the
	// v2 testnet at all — it would have thrown at startup even once deployed.
	describe("chain 55516 (testnet v2)", () => {
		const v2Env = (): NodeJS.ProcessEnv => ({
			...validEnv(),
			CHAIN_ID: "55516",
			ZERODEV_SPONSORSHIP_RPC_URL: "https://rpc.zerodev.app/api/v3/proj/chain/55516",
		})

		test("is accepted", () => {
			const c = parseConfig(v2Env())
			expect(c.chainId).toBe(55516)
			expect(c.zerodevSponsorshipRpcUrl).toBe("https://rpc.zerodev.app/api/v3/proj/chain/55516")
		})

		test("requires ZERODEV_SPONSORSHIP_RPC_URL — there is no Safe/Pimlico fallback there", () => {
			const env = v2Env()
			delete env.ZERODEV_SPONSORSHIP_RPC_URL
			expect(() => parseConfig(env)).toThrow(ConfigError)
		})

		test("does NOT require PIMLICO_API_KEY — that chain never calls Pimlico", () => {
			const env = v2Env()
			delete env.PIMLICO_API_KEY
			const c = parseConfig(env)
			expect(c.chainId).toBe(55516)
			expect(c.pimlicoApiKey).toBe("")
		})
	})

	test("PIMLICO_API_KEY is still required on the Safe chains", () => {
		const env = validEnv()
		delete env.PIMLICO_API_KEY
		env.CHAIN_ID = "19411"
		expect(() => parseConfig(env)).toThrow(ConfigError)
	})

	test("ZERODEV_SPONSORSHIP_RPC_URL is ignored on the Safe chains", () => {
		const c = parseConfig({...validEnv(), CHAIN_ID: "19411", ZERODEV_SPONSORSHIP_RPC_URL: "https://ignored"})
		expect(c.zerodevSponsorshipRpcUrl).toBeUndefined()
	})

	test("throws on an out-of-range PORT", () => {
		expect(() => parseConfig({...validEnv(), PORT: "70000"})).toThrow(ConfigError)
	})
})
