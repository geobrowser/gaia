/** replay-edit opens no Kafka connection, so loadConfig(env, {kafka: false}) must not demand the KAFKA_* variables. */
import {describe, expect, it} from "vitest"
import {loadConfig} from "../config.js"

const base = {GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}

describe("loadConfig kafka option", () => {
	it("still requires KAFKA_* by default", () => {
		expect(() => loadConfig(base)).toThrow("KAFKA_BROKER is required")
	})
	it("does not require KAFKA_* when kafka is false", () => {
		const c = loadConfig(base, {kafka: false})
		expect(c.kafka.broker).toBe("")
		expect(c.keyPath).toBe("k")
	})
	it("passes KAFKA_* through when set and kafka is false", () => {
		expect(loadConfig({...base, KAFKA_BROKER: "b"}, {kafka: false}).kafka.broker).toBe("b")
	})
})
