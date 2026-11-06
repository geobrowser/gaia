/** Minimal protobuf reader for knowledge.HermesEdit (hermes-schema/proto/knowledge.proto). */

export type HermesEdit = {
	id?: string
	name?: string
	payload?: Uint8Array
	authors: string[]
	spaceId?: string
	isCanonical: boolean
	createdAt: number | null
	/** blockchain_metadata.block_number (field 8, subfield 3), or null when absent. */
	blockNumber: number | null
}

type Field = [number, bigint | Uint8Array]

function varint(b: Uint8Array, p: {i: number}): bigint {
	let r = 0n
	let s = 0n
	for (;;) {
		const c = b[p.i++]
		if (c === undefined) throw new Error("truncated varint")
		r |= BigInt(c & 0x7f) << s
		if (!(c & 0x80)) break
		s += 7n
	}
	return r
}

export function parseFields(b: Uint8Array): Field[] {
	const p = {i: 0}
	const out: Field[] = []
	while (p.i < b.length) {
		const tag = Number(varint(b, p))
		const f = tag >> 3
		const w = tag & 7
		if (w === 0) out.push([f, varint(b, p)])
		else if (w === 2) {
			const l = Number(varint(b, p))
			if (p.i + l > b.length) throw new Error("truncated field")
			out.push([f, b.subarray(p.i, p.i + l)])
			p.i += l
		} else if (w === 1) {
			out.push([f, b.subarray(p.i, p.i + 8)])
			p.i += 8
		} else if (w === 5) {
			out.push([f, b.subarray(p.i, p.i + 4)])
			p.i += 4
		} else throw new Error(`wire type ${w}`)
	}
	return out
}

const hex = (v: Uint8Array) => Buffer.from(v).toString("hex")

export function parseHermesEdit(b: Uint8Array): HermesEdit {
	const e: HermesEdit = {authors: [], isCanonical: false, createdAt: null, blockNumber: null}
	for (const [f, v] of parseFields(b)) {
		if (v instanceof Uint8Array) {
			if (f === 1) e.id = hex(v)
			else if (f === 2) e.name = Buffer.from(v).toString("utf8")
			else if (f === 3) e.payload = v
			else if (f === 4) e.authors.push(hex(v))
			else if (f === 6) e.spaceId = hex(v)
			else if (f === 8) {
				for (const [k, x] of parseFields(v)) {
					if (k === 1 && typeof x === "bigint") e.createdAt = Number(x)
					else if (k === 3 && typeof x === "bigint") e.blockNumber = Number(x)
				}
			}
		} else if (f === 7) e.isCanonical = v === 1n
	}
	return e
}
