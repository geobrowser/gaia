import {describe, expect, it} from "vitest"
import {baseTimestampOf, recordBaseTimestamp, withBaseTimestampOf} from "../proposal-diff"

describe("the base timestamp kept beside a diff", () => {
	it("is absent for a diff that recorded none, and stays so through a copy", () => {
		const diff = {entities: []}
		expect(baseTimestampOf(diff)).toBeUndefined()
		expect(baseTimestampOf(withBaseTimestampOf({...diff}, diff))).toBeUndefined()
	})

	it("follows the copy a route makes, and never reaches the serialized body", () => {
		const diff = {entities: []}
		recordBaseTimestamp(diff, 1_700_000_000n)
		const copy = withBaseTimestampOf({...diff, names: []}, diff)
		expect(baseTimestampOf(diff)).toBe(1_700_000_000n)
		expect(baseTimestampOf(copy)).toBe(1_700_000_000n)
		expect(JSON.stringify(copy)).toBe('{"entities":[],"names":[]}')
		expect(copy).toEqual({entities: [], names: []})
	})
})
