import {describe, expect, test} from "bun:test"

import {ConfigError, parseConfig} from "../src/config.js"

describe("parseConfig", () => {
	test("parses a valid environment with default port", () => {
		const config = parseConfig({GEO_WEBHOOK_SECRET: "s3cr3t"})
		expect(config).toEqual({port: 8080, webhookSecret: "s3cr3t"})
	})

	test("honors a valid PORT", () => {
		const config = parseConfig({GEO_WEBHOOK_SECRET: "s3cr3t", PORT: "3000"})
		expect(config.port).toBe(3000)
	})

	test("trims the secret", () => {
		const config = parseConfig({GEO_WEBHOOK_SECRET: "  s3cr3t  "})
		expect(config.webhookSecret).toBe("s3cr3t")
	})

	test("throws when GEO_WEBHOOK_SECRET is missing", () => {
		expect(() => parseConfig({})).toThrow(ConfigError)
	})

	test("throws when GEO_WEBHOOK_SECRET is blank", () => {
		expect(() => parseConfig({GEO_WEBHOOK_SECRET: "   "})).toThrow(ConfigError)
	})

	test("throws on a non-numeric PORT", () => {
		expect(() => parseConfig({GEO_WEBHOOK_SECRET: "s", PORT: "abc"})).toThrow(ConfigError)
	})

	test("throws on an out-of-range PORT", () => {
		expect(() => parseConfig({GEO_WEBHOOK_SECRET: "s", PORT: "70000"})).toThrow(ConfigError)
	})
})
