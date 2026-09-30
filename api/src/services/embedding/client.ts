/**
 * Client for gaia's embedding-service (see embedding-service/README.md).
 *
 * The API embeds only *queries* with it; documents are embedded by the embedding-indexer.
 * Every response names the descriptor hash of the slot that produced the vectors, and callers
 * compare it with the hash the index registered — nothing about an embedding is implicit.
 */

export type EmbeddingPurpose = "document" | "query"

export interface SlotInfo {
	/** The full descriptor as the service loaded it. */
	descriptor: Record<string, unknown>
	descriptor_hash: string
	vector_field: string
}

export interface ServiceInfo {
	slots: Record<string, SlotInfo>
}

export interface EmbedResult {
	descriptorHash: string
	dimensions: number
	vectors: number[][]
	tookMs: number
}

export class EmbeddingServiceError extends Error {
	constructor(
		message: string,
		public readonly status?: number,
	) {
		super(message)
		this.name = "EmbeddingServiceError"
	}
}

export class EmbeddingServiceClient {
	private readonly baseUrl: string

	constructor(
		baseUrl: string,
		private readonly timeoutMs: number = 2000,
		private readonly fetchImpl: typeof fetch = fetch,
	) {
		this.baseUrl = baseUrl.replace(/\/+$/, "")
	}

	async info(): Promise<ServiceInfo> {
		const res = await this.fetchImpl(`${this.baseUrl}/info`, {signal: AbortSignal.timeout(this.timeoutMs)})
		if (!res.ok) throw new EmbeddingServiceError(`embedding-service /info returned ${res.status}`, res.status)
		return (await res.json()) as ServiceInfo
	}

	async embed(slot: string, purpose: EmbeddingPurpose, texts: string[]): Promise<EmbedResult> {
		const res = await this.fetchImpl(`${this.baseUrl}/embed`, {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({slot, purpose, texts}),
			signal: AbortSignal.timeout(this.timeoutMs),
		})
		if (!res.ok) {
			let detail = ""
			try {
				const body = (await res.json()) as {error?: {code?: string; message?: string}}
				detail = body.error ? ` ${body.error.code}: ${body.error.message}` : ""
			} catch {
				// no JSON body
			}
			throw new EmbeddingServiceError(`embedding-service /embed returned ${res.status}${detail}`, res.status)
		}
		const body = (await res.json()) as {
			descriptor_hash: string
			dimensions: number
			vectors: number[][]
			took_ms: number
		}
		return {
			descriptorHash: body.descriptor_hash,
			dimensions: body.dimensions,
			vectors: body.vectors,
			tookMs: body.took_ms,
		}
	}
}
