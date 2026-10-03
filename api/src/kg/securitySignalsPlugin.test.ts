import {describe, expect, it} from "vitest"
import {renderSecuritySignalsPrometheus, useSecuritySignals} from "./securitySignalsPlugin"

describe("renderSecuritySignalsPrometheus", () => {
	it("exports nothing until the plugin is in use, so a removed plugin trips ApiSecuritySignalsMissing", () => {
		// This file never imports postgraphile, so nothing has registered the plugin yet.
		expect(renderSecuritySignalsPrometheus()).toBe("")
		useSecuritySignals()
		expect(renderSecuritySignalsPrometheus()).toContain("gaia_api_graphql_rejected_total")
	})
})
