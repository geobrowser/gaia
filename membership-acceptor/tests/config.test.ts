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
		expect([...c.autoacceptSpaceIds]).toEqual(["d4f5a6b7-0000-0000-0000-000000000000"])
	})

	test("adds the 0x prefix to a bare private key", () => {
		const c = parseConfig({...validEnv(), ACCEPTOR_PRIVATE_KEY: "1".repeat(64)})
		expect(c.acceptorPrivateKey).toBe(`0x${"1".repeat(64)}`)
	})

	test("lowercases and de-duplicates the allowlist, dropping blanks", () => {
		const c = parseConfig({...validEnv(), MEMBERSHIP_AUTOACCEPT_SPACE_IDS: " AAA , aaa ,, BBB "})
		expect([...c.autoacceptSpaceIds].sort()).toEqual(["aaa", "bbb"])
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

	test("throws on an out-of-range PORT", () => {
		expect(() => parseConfig({...validEnv(), PORT: "70000"})).toThrow(ConfigError)
	})
})
