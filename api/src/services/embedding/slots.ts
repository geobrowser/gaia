/**
 * The slot registry: which embedding slots the index carries (its `_meta`), which of them the
 * running embedding-service actually serves with the *same* descriptor, and which is the default.
 *
 * A slot is "ready" only when both sides agree, compared on the canonical JSON of the descriptor
 * (keys sorted at every level, no whitespace — the same form the Rust side hashes). A slot the
 * index lists but the service does not load, or loads with a different descriptor, is reported
 * and never used.
 */

import {SearchError} from "../search/types"
import type {EmbeddingServiceClient} from "./client"

export interface ReadySlot {
	id: string
	vectorField: string
	descriptorHash: string
	dimensions: number
	modelId: string
	/** Default `min_score` for this slot on the (1 + cos) / 2 scale. */
	scoreFloor: number
}

export interface SlotStatus {
	id: string
	state: "ready" | "not_loaded" | "descriptor_mismatch" | "invalid"
	detail?: string
}

/** Canonical JSON: sorted keys recursively, compact. Mirrors `embedding::descriptor::canonicalize`. */
export function canonicalJson(value: unknown): string {
	if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`
	if (value !== null && typeof value === "object") {
		const obj = value as Record<string, unknown>
		return `{${Object.keys(obj)
			.sort()
			.map((k) => `${JSON.stringify(k)}:${canonicalJson(obj[k])}`)
			.join(",")}}`
	}
	return JSON.stringify(value)
}

export interface SlotRegistryDeps {
	/** The index mapping's `_meta` object (or undefined when absent). */
	readMeta: () => Promise<Record<string, unknown> | undefined>
	service: Pick<EmbeddingServiceClient, "info">
	// Method signatures (bivariant) so the API's telemetry logger, whose context type is narrower
	// than `object`, is assignable.
	log?: {
		info(msg: string, ctx?: Record<string, unknown>): void
		warn(msg: string, ctx?: Record<string, unknown>): void
	}
}

export class SlotRegistry {
	private ready = new Map<string, ReadySlot>()
	private statuses: SlotStatus[] = []
	private defaultSlot: string | undefined
	private timer: ReturnType<typeof setInterval> | undefined
	public lastError: string | undefined
	public lastRefreshAt: Date | undefined

	constructor(private readonly deps: SlotRegistryDeps) {}

	/** Re-read the index `_meta` and the service `/info`, and recompute which slots are usable. */
	async refresh(): Promise<void> {
		try {
			const [meta, info] = await Promise.all([this.deps.readMeta(), this.deps.service.info()])
			const registered = ((meta?.embedding_slots as Record<string, unknown> | undefined) ?? {}) as Record<
				string,
				Record<string, unknown>
			>
			const ready = new Map<string, ReadySlot>()
			const statuses: SlotStatus[] = []
			for (const [id, indexDescriptor] of Object.entries(registered)) {
				const loaded = info.slots[id]
				if (!loaded) {
					statuses.push({
						id,
						state: "not_loaded",
						detail: "registered on the index, not loaded by embedding-service",
					})
					continue
				}
				if (canonicalJson(loaded.descriptor) !== canonicalJson(indexDescriptor)) {
					statuses.push({
						id,
						state: "descriptor_mismatch",
						detail: `index and embedding-service disagree on the descriptor (service hash ${loaded.descriptor_hash})`,
					})
					continue
				}
				const dims = Number(indexDescriptor.dimensions)
				const floor = Number(indexDescriptor.score_floor)
				if (!Number.isFinite(dims) || dims <= 0 || !Number.isFinite(floor)) {
					statuses.push({id, state: "invalid", detail: "descriptor lacks dimensions or score_floor"})
					continue
				}
				ready.set(id, {
					id,
					vectorField: loaded.vector_field || `emb_${id}`,
					descriptorHash: loaded.descriptor_hash,
					dimensions: dims,
					modelId: String(indexDescriptor.model_id ?? ""),
					scoreFloor: floor,
				})
				statuses.push({id, state: "ready"})
			}
			const def = meta?.embedding_default_slot
			this.ready = ready
			this.statuses = statuses
			this.defaultSlot = typeof def === "string" ? def : undefined
			this.lastError = undefined
			this.lastRefreshAt = new Date()
			this.deps.log?.info("embedding slots refreshed", {
				ready: [...ready.keys()],
				default: this.defaultSlot,
				notReady: statuses.filter((s) => s.state !== "ready"),
			})
		} catch (error) {
			this.lastError = error instanceof Error ? error.message : String(error)
			this.deps.log?.warn("embedding slots refresh failed; keeping the previous registry", {
				error: this.lastError,
			})
		}
	}

	/** Refresh on an interval. The timer never keeps the process alive. */
	start(intervalMs: number): void {
		this.stop()
		this.timer = setInterval(() => void this.refresh(), intervalMs)
		this.timer.unref?.()
	}

	stop(): void {
		if (this.timer) clearInterval(this.timer)
		this.timer = undefined
	}

	readySlots(): ReadySlot[] {
		return [...this.ready.values()]
	}

	status(): {default?: string; slots: SlotStatus[]; lastError?: string; lastRefreshAt?: Date} {
		return {
			default: this.defaultSlot,
			slots: this.statuses,
			lastError: this.lastError,
			lastRefreshAt: this.lastRefreshAt,
		}
	}

	/**
	 * The slot a request should use. An unknown or not-ready slot named explicitly is a client
	 * error; no usable default at all means semantic search is unavailable right now.
	 */
	resolve(requested?: string): ReadySlot {
		if (requested) {
			const slot = this.ready.get(requested)
			if (slot) return slot
			const known = this.statuses.find((s) => s.id === requested)
			if (known) {
				throw SearchError.unavailable(
					`embedding slot ${requested} is registered but not servable: ${known.detail ?? known.state}`,
				)
			}
			throw SearchError.validationError(`unknown embedding slot '${requested}'`)
		}
		const id = this.defaultSlot
		const slot = id ? this.ready.get(id) : undefined
		if (slot) return slot
		if (this.ready.size === 0) {
			throw SearchError.unavailable(
				this.lastError
					? `no embedding slot is servable (last refresh failed: ${this.lastError})`
					: "no embedding slot is servable (none registered on the index, or none loaded by embedding-service)",
			)
		}
		throw SearchError.unavailable(
			id
				? `default embedding slot ${id} is not servable`
				: "the index has embedding slots but no default; pass slot=",
		)
	}
}
