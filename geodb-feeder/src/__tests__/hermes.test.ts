import {describe, expect, it} from "vitest"
import {parseHermesEdit} from "../hermes.js"

const varint = (n: number) => {
	const out: number[] = []
	let v = n
	while (v > 0x7f) {
		out.push((v & 0x7f) | 0x80)
		v >>>= 7
	}
	out.push(v)
	return out
}
const bytes = (field: number, data: Uint8Array | number[]) => [...varint((field << 3) | 2), ...varint(data.length), ...data]

describe("parseHermesEdit", () => {
	it("reads id, payload, authors, space, canonical flag and creation time", () => {
		const id = Array.from({length: 16}, (_, i) => i)
		const space = Array.from({length: 16}, (_, i) => 255 - i)
		const created = [...varint(1 << 3), ...varint(1_700_000_000)]
		const msg = Uint8Array.from([
			...bytes(1, id),
			...bytes(2, new TextEncoder().encode("name")),
			...bytes(3, [9, 9, 9]),
			...bytes(4, [1]),
			...bytes(4, [2]),
			...bytes(6, space),
			...varint(7 << 3),
			1,
			...bytes(8, created),
		])
		const e = parseHermesEdit(msg)
		expect(e.id).toBe(Buffer.from(id).toString("hex"))
		expect(e.name).toBe("name")
		expect([...(e.payload ?? [])]).toEqual([9, 9, 9])
		expect(e.authors).toEqual(["01", "02"])
		expect(e.spaceId).toBe(Buffer.from(space).toString("hex"))
		expect(e.isCanonical).toBe(true)
		expect(e.createdAt).toBe(1_700_000_000)
		expect(e.blockNumber).toBeNull()
	})

	it("reads the block number from the blockchain metadata (field 8, subfield 3)", () => {
		expect(parseHermesEdit(Uint8Array.from([0x42, 0x04, 0x08, 0x05, 0x18, 0x07])).blockNumber).toBe(7)
	})

	it("rejects truncated input", () => {
		expect(() => parseHermesEdit(Uint8Array.from([0x1a, 0x05, 1]))).toThrow()
	})
})
