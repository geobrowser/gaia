import {readFileSync} from "node:fs"
import {describe, expect, it} from "vitest"

// main.ts builds the deps of the entitiesConnection / entitiesOrderedByProperty(Connection) shadows by hand, and a flag it
// forgets is off in production whatever its env says: GEODB_COUNT_ONLY_SHADOW=on was set live while every totalCount-only
// entitiesConnection was still counted `unsupported_selection{count_only}`. main.ts cannot be imported in a test, so the
// wiring is held to its source.
describe("GEODB_COUNT_ONLY_SHADOW wiring", () => {
	it("reaches the entitiesConnection shadow's deps", () => {
		const main = readFileSync(new URL("../../../main.ts", import.meta.url), "utf8")
		const start = main.indexOf("[ENTITIES_CONNECTION_FIELD, createEntitiesConnectionShadow]")
		expect(start).toBeGreaterThan(0)
		const block = main.slice(start, start + 3500)
		expect(block).toContain("countOnly: geoDbCardConfig.countOnly === true")
	})
})
