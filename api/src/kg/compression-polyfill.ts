import {Readable, Writable} from "node:stream"
import zlib from "node:zlib"

const transformMap = {
	deflate: zlib.createDeflate,
	"deflate-raw": zlib.createDeflateRaw,
	gzip: zlib.createGzip,
}

globalThis.CompressionStream = class CompressionStream {
	readable: ReadableStream
	writable: WritableStream

	constructor(format: keyof typeof transformMap) {
		const handle = transformMap[format]()
		// @ts-expect-error idk
		this.readable = Readable.toWeb(handle)
		this.writable = Writable.toWeb(handle)
	}
}
