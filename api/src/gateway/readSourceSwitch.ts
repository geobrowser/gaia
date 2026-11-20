/**
 * Runtime per-field read-source switch with durable, shared sliding-window auto-revert back to Postgres.
 *
 * Reads per-field modes ("off" | "shadow" | "on") and shared auto-revert latches from a mounted
 * Kubernetes ConfigMap file (`GEODB_READ_SOURCE_PATH`, e.g. `/etc/geodb/read-source/read-source.json`)
 * without requiring a pod restart or deploy. Re-checks the file with a single `stat` at most every
 * `refreshMs` (default 2 s) and keeps the last known good config (or startup env defaults) if the file
 * is missing or malformed.
 *
 * Covers both GraphQL gateway root fields (`GRAPHQL_GATEWAY_FIELDS`) and the REST entity-card route
 * (`ENTITY_CARDS_FIELD` / `GEODB_ENTITY_CARDS`).
 *
 * Auto-revert: while a field is serving from geo-db ("on"), each served-field operation outcome is
 * recorded in a per-field sliding window using the same thresholds and failure classes as
 * `monitoring/k8s/v2/geodb-gateway-serve-alerts.yaml` (GatewayServeFallbackHigh / GatewayServeLatencyHigh):
 * - Failure-class fallback reasons (`timeout`, `overloaded`, `client_failed`, `bad_request`, `error`, `id_map`)
 *   over 10% of at least 20 operations in 15m, OR
 * - Served p99 latency over 250 ms across at least 20 served operations in 15m.
 *
 * When a field trips, it latches immediately in memory (effective mode "off"), logs once, bumps
 * `gaia_api_gateway_read_source_auto_revert_total{field,reason}` once, and patches
 * `"reverted": {"<field>": "<iso-ts> <reason>"}` into ConfigMap `geodb-read-source` so every replica
 * and any restarted pod honours the latch. A person resets by deleting the field's key under
 * `"reverted"` in the ConfigMap (or calling `reset()` in tests).
 */

import {readFileSync} from "node:fs"
import {readFile, stat} from "node:fs/promises"
import {request} from "node:https"
import {type FieldMode, parseFieldMode} from "./config"

export const ENTITY_CARDS_FIELD = "entityCards"
export const READ_SOURCE_CONFIGMAP_KEY = "read-source.json"

export const AUTO_REVERT_REASONS = ["error_rate", "latency"] as const
export type AutoRevertReason = (typeof AUTO_REVERT_REASONS)[number]

/**
 * Failure-class fallback reasons, matching monitoring/k8s/v2/geodb-gateway-serve-alerts.yaml
 * (GatewayServeFallbackHigh): geo-db failing a query it could have answered, or the reverse id map failing.
 */
export const GEO_DB_ERROR_REASONS = new Set([
	"timeout",
	"overloaded",
	"client_failed",
	"bad_request",
	"error",
	"id_map",
])

export function isGeoDbErrorReason(reason: string): boolean {
	return GEO_DB_ERROR_REASONS.has(reason)
}

export type SwitchLogger = {
	info: (message: string, data?: Record<string, unknown>) => void
	warn: (message: string, data?: Record<string, unknown>) => void
}

export type AutoRevertConfig = {
	/** Sliding window duration in milliseconds (default 15m, matching GatewayServeFallbackHigh / GatewayServeLatencyHigh). */
	windowMs: number
	/** Minimum operations in the window before auto-revert can trip (default 20). */
	minSamples: number
	/** Maximum allowed failure-class fallback fraction over served_total in [0, 1] (default 0.10). */
	maxErrorRate: number
	/** Maximum allowed served p99 latency in ms (default 250). */
	maxLatencyMs: number
}

export const DEFAULT_AUTO_REVERT_CONFIG: AutoRevertConfig = {
	windowMs: 15 * 60_000,
	minSamples: 20,
	maxErrorRate: 0.1,
	maxLatencyMs: 250,
}

const DEFAULT_REFRESH_MS = 2_000
const MAX_WINDOW_SAMPLES = 4_096
const SA = "/var/run/secrets/kubernetes.io/serviceaccount"

const positiveInt = (raw: string | undefined, fallback: number, max: number): number => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

const unitFraction = (raw: string | undefined, fallback: number): number => {
	if (raw === undefined || raw.trim() === "") return fallback
	const n = Number(raw)
	return Number.isFinite(n) && n >= 0 && n <= 1 ? n : fallback
}

export function readAutoRevertConfig(env: Record<string, string | undefined>): AutoRevertConfig {
	return {
		windowMs: positiveInt(env.GEODB_READ_SOURCE_WINDOW_MS, DEFAULT_AUTO_REVERT_CONFIG.windowMs, 3_600_000),
		minSamples: positiveInt(
			env.GEODB_READ_SOURCE_MIN_SAMPLES,
			DEFAULT_AUTO_REVERT_CONFIG.minSamples,
			MAX_WINDOW_SAMPLES,
		),
		maxErrorRate: unitFraction(env.GEODB_READ_SOURCE_MAX_ERROR_RATE, DEFAULT_AUTO_REVERT_CONFIG.maxErrorRate),
		maxLatencyMs: positiveInt(
			env.GEODB_READ_SOURCE_MAX_LATENCY_MS,
			DEFAULT_AUTO_REVERT_CONFIG.maxLatencyMs,
			60_000,
		),
	}
}

export type ReadOutcomeKind = "served" | "failure" | "declined"

export type ReadOutcome = {
	/** "served" = answered by geo-db, "failure" = failure-class fallback, "declined" = translator decline (denominator only). */
	kind: ReadOutcomeKind
	latencyMs: number
}

export type RevertedState = {
	reason: AutoRevertReason
	at: number
	errorRate: number
	p99LatencyMs: number
	samples: number
}

export type PersistRevert = (field: string, entry: string) => Promise<void>

export type ReadSourceSwitch = {
	/** Refresh from the watched ConfigMap file if at least `refreshMs` have elapsed since the last check. */
	refreshIfDue(): Promise<void>
	/** Effective mode for a field: "off" while auto-reverted (locally or in ConfigMap), otherwise the configured mode. */
	effectiveMode(field: string): FieldMode
	/** Configured mode from the ConfigMap (or startup default), ignoring auto-revert state. */
	configuredMode(field: string): FieldMode
	/** True while the field is latched back to Postgres after an auto-revert. */
	isReverted(field: string): boolean
	/** Record a served-field operation outcome and trip auto-revert if thresholds are crossed. */
	recordOutcome(field: string, outcome: ReadOutcome): void
	/** Manually clear an auto-revert latch and sliding window for a field. */
	reset(field: string): void
}

export type ReadSourceSwitchOptions = {
	/** Path to the mounted ConfigMap JSON file; empty/undefined disables file watching and uses defaults. */
	path?: string
	/** Startup defaults (from GRAPHQL_GATEWAY_FIELDS and GEODB_ENTITY_CARDS). */
	defaults?: ReadonlyMap<string, FieldMode>
	/** Minimum interval between `stat` checks, in ms (default 2000). */
	refreshMs?: number
	/** Injectable clock for tests. */
	now?: () => number
	/** Logger for one-time auto-revert warnings. */
	log?: SwitchLogger
	/** Sliding-window auto-revert thresholds. */
	autoRevert?: Partial<AutoRevertConfig>
	/** Persists a field's auto-revert latch to the shared ConfigMap (`reverted.<field> = "<ts> <reason>"`). */
	persistRevert?: PersistRevert
}

type Sample = {at: number; kind: ReadOutcomeKind; latencyMs: number}

type ParsedFile = {
	fields: Map<string, FieldMode>
	reverted: Map<string, string>
	resets: Map<string, string>
}

function parseReadSourceFile(raw: string): ParsedFile {
	const parsed = JSON.parse(raw) as unknown
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("read-source config must be a JSON object")
	}
	const obj = parsed as Record<string, unknown>
	const fields = new Map<string, FieldMode>()
	if (obj.fields && typeof obj.fields === "object" && !Array.isArray(obj.fields)) {
		for (const [k, v] of Object.entries(obj.fields as Record<string, unknown>)) {
			const name = k.trim()
			if (name && /^[_A-Za-z][_0-9A-Za-z]*$/.test(name)) {
				fields.set(name, parseFieldMode(typeof v === "string" ? v : undefined))
			}
		}
	}
	const reverted = new Map<string, string>()
	if (obj.reverted && typeof obj.reverted === "object" && !Array.isArray(obj.reverted)) {
		for (const [k, v] of Object.entries(obj.reverted as Record<string, unknown>)) {
			const name = k.trim()
			const entry = typeof v === "string" ? v.trim() : ""
			if (name && entry) reverted.set(name, entry)
		}
	}
	const resets = new Map<string, string>()
	if (obj.reset && typeof obj.reset === "object" && !Array.isArray(obj.reset)) {
		for (const [k, v] of Object.entries(obj.reset as Record<string, unknown>)) {
			const name = k.trim()
			const token = typeof v === "string" ? v.trim() : ""
			if (name && token) resets.set(name, token)
		}
	}
	return {fields, reverted, resets}
}

/**
 * Patches `"reverted": {"<field>": "<ts> <reason>"}` inside `read-source.json` of ConfigMap `name`
 * using the pod's ServiceAccount (requires a Role scoped to `resourceNames: [name]` with verbs `get`
 * and `patch`). Returns null outside a Kubernetes cluster or when `name` is empty.
 */
export function kubeConfigMapPatchRevert(name: string, key: string = READ_SOURCE_CONFIGMAP_KEY): PersistRevert | null {
	const host = process.env.KUBERNETES_SERVICE_HOST
	if (!name.trim() || !host) return null
	const port = process.env.KUBERNETES_SERVICE_PORT ?? "443"
	const call = (method: string, reqPath: string, body?: unknown, contentType?: string) =>
		new Promise<{status: number; text: string}>((resolve, reject) => {
			const payload = body === undefined ? undefined : JSON.stringify(body)
			const headers: Record<string, string | number> = {
				authorization: `Bearer ${readFileSync(`${SA}/token`, "utf8").trim()}`,
			}
			if (payload !== undefined && contentType) {
				headers["content-type"] = contentType
				headers["content-length"] = Buffer.byteLength(payload)
			}
			const req = request(
				{host, port, path: reqPath, method, ca: readFileSync(`${SA}/ca.crt`), headers, timeout: 10_000},
				(res) => {
					let text = ""
					res.on("data", (d) => {
						text += d
					})
					res.on("end", () => resolve({status: res.statusCode ?? 0, text}))
				},
			)
			req.on("error", reject)
			req.on("timeout", () => req.destroy(new Error("kube api timeout")))
			if (payload !== undefined) req.end(payload)
			else req.end()
		})
	return async (field, entry) => {
		const namespace = readFileSync(`${SA}/namespace`, "utf8").trim()
		const url = `/api/v1/namespaces/${namespace}/configmaps/${name.trim()}`
		const current = await call("GET", url)
		if (current.status !== 200) {
			throw new Error(`configmap get: HTTP ${current.status} ${current.text.slice(0, 200)}`)
		}
		const cm = JSON.parse(current.text) as {data?: Record<string, string>}
		const existingRaw = cm.data?.[key] ?? "{}"
		const doc = JSON.parse(existingRaw) as Record<string, unknown>
		const reverted =
			doc.reverted && typeof doc.reverted === "object" && !Array.isArray(doc.reverted)
				? {...(doc.reverted as Record<string, string>)}
				: {}
		reverted[field] = entry
		const nextDoc = {...doc, reverted}
		const patch = await call(
			"PATCH",
			url,
			{data: {[key]: `${JSON.stringify(nextDoc, null, 2)}\n`}},
			"application/merge-patch+json",
		)
		if (patch.status !== 200) {
			throw new Error(`configmap patch: HTTP ${patch.status} ${patch.text.slice(0, 200)}`)
		}
	}
}

function p99(values: number[]): number {
	if (values.length === 0) return 0
	const sorted = [...values].sort((a, b) => a - b)
	const idx = Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.99) - 1)
	return sorted[idx] ?? 0
}

// Module-level Prometheus metrics rendered by /health/metrics.
const trackedFields = new Set<string>()
const autoReverts = new Map<string, number>()
const revertedGauge = new Map<string, number>()

const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

export function renderReadSourceSwitchMetrics(): string {
	if (trackedFields.size === 0 && autoReverts.size === 0) return ""
	const fields = new Set<string>(trackedFields)
	for (const key of autoReverts.keys()) {
		const field = key.split("\t")[0]
		if (field) fields.add(field)
	}
	const lines = [
		"# HELP gaia_api_gateway_read_source_auto_revert_total Times a field automatically reverted from geo-db back to Postgres, by field and reason.",
		"# TYPE gaia_api_gateway_read_source_auto_revert_total counter",
	]
	for (const field of fields) {
		for (const reason of AUTO_REVERT_REASONS) {
			lines.push(
				`gaia_api_gateway_read_source_auto_revert_total{field="${field}",reason="${reason}"} ${autoReverts.get(`${field}\t${reason}`) ?? 0}`,
			)
		}
	}
	lines.push(
		"# HELP gaia_api_gateway_read_source_reverted Whether a field is currently latched on Postgres after an auto-revert (1 = reverted, 0 = normal).",
		"# TYPE gaia_api_gateway_read_source_reverted gauge",
	)
	for (const field of fields) {
		lines.push(`gaia_api_gateway_read_source_reverted{field="${field}"} ${revertedGauge.get(field) ?? 0}`)
	}
	return `${lines.join("\n")}\n`
}

/** Reset accumulated metrics state. Tests only. */
export function __resetReadSourceSwitchMetricsForTests(): void {
	trackedFields.clear()
	autoReverts.clear()
	revertedGauge.clear()
}

export function createReadSourceSwitch(options: ReadSourceSwitchOptions = {}): ReadSourceSwitch {
	const filePath = options.path?.trim() ?? ""
	const defaults = new Map<string, FieldMode>(options.defaults ?? [])
	const refreshMs = options.refreshMs ?? DEFAULT_REFRESH_MS
	const now = options.now ?? Date.now
	const cfg: AutoRevertConfig = {
		...DEFAULT_AUTO_REVERT_CONFIG,
		...options.autoRevert,
	}

	let fileFields = new Map<string, FieldMode>()
	let sharedReverted = new Map<string, string>()
	let stamp: string | null = null
	let checkedAt = Number.NEGATIVE_INFINITY
	let inflight: Promise<void> | null = null

	const acknowledgedResets = new Map<string, string>()
	const windows = new Map<string, Sample[]>()
	const localReverted = new Map<string, RevertedState>()

	const resetField = (field: string) => {
		localReverted.delete(field)
		sharedReverted.delete(field)
		windows.delete(field)
		revertedGauge.set(field, 0)
	}

	const reload = async () => {
		const info = await stat(filePath)
		const nextStamp = `${info.mtimeMs}:${info.size}`
		if (stamp === nextStamp) return
		const parsed = parseReadSourceFile(await readFile(filePath, "utf8"))
		fileFields = parsed.fields
		stamp = nextStamp

		// Reconcile durable shared latches from the ConfigMap's `reverted` object.
		const prevShared = sharedReverted
		sharedReverted = parsed.reverted
		for (const field of sharedReverted.keys()) {
			trackedFields.add(field)
			revertedGauge.set(field, 1)
			// Once the latch is observed in the ConfigMap, drop the local pending copy so deleting the key resets it.
			localReverted.delete(field)
		}
		for (const field of prevShared.keys()) {
			if (!sharedReverted.has(field)) {
				// A person deleted the field's key under `reverted` in the ConfigMap.
				localReverted.delete(field)
				windows.delete(field)
				revertedGauge.set(field, 0)
			}
		}

		for (const [field, token] of parsed.resets) {
			if (acknowledgedResets.get(field) !== token) {
				acknowledgedResets.set(field, token)
				resetField(field)
			}
		}
	}

	const refreshIfDue = async () => {
		if (!filePath) return
		if (now() - checkedAt >= refreshMs) {
			checkedAt = now()
			inflight ??= reload()
				.catch(() => {
					// Keep last known good config (or startup defaults) when the file is missing or malformed.
				})
				.finally(() => {
					inflight = null
				})
		}
		if (inflight) await inflight
	}

	const configuredMode = (field: string): FieldMode => fileFields.get(field) ?? defaults.get(field) ?? "off"

	const isReverted = (field: string): boolean => localReverted.has(field) || sharedReverted.has(field)

	const effectiveMode = (field: string): FieldMode => (isReverted(field) ? "off" : configuredMode(field))

	const recordOutcome = (field: string, outcome: ReadOutcome): void => {
		if (isReverted(field)) return
		trackedFields.add(field)
		const ts = now()
		const cutoff = ts - cfg.windowMs
		const list = windows.get(field) ?? []
		list.push({at: ts, kind: outcome.kind, latencyMs: outcome.latencyMs})
		let start = 0
		while (start < list.length && (list[start]?.at ?? 0) < cutoff) start++
		if (start > 0) list.splice(0, start)
		if (list.length > MAX_WINDOW_SAMPLES) list.splice(0, list.length - MAX_WINDOW_SAMPLES)
		windows.set(field, list)

		if (list.length < cfg.minSamples) return

		let failures = 0
		const servedLatencies: number[] = []
		for (const s of list) {
			if (s.kind === "failure") failures++
			else if (s.kind === "served") servedLatencies.push(s.latencyMs)
		}
		const errorRate = failures / list.length
		const p99LatencyMs = p99(servedLatencies)

		let reason: AutoRevertReason | null = null
		if (errorRate > cfg.maxErrorRate) reason = "error_rate"
		else if (servedLatencies.length >= cfg.minSamples && p99LatencyMs > cfg.maxLatencyMs) reason = "latency"

		if (!reason) return

		localReverted.set(field, {reason, at: ts, errorRate, p99LatencyMs, samples: list.length})
		revertedGauge.set(field, 1)
		bump(autoReverts, `${field}\t${reason}`)
		options.log?.warn("geodb read source auto-reverted to postgres", {
			field,
			reason,
			errorRate: Number(errorRate.toFixed(4)),
			p99LatencyMs: Math.round(p99LatencyMs),
			samples: list.length,
			windowMs: cfg.windowMs,
		})
		if (options.persistRevert) {
			const entry = `${new Date(ts).toISOString()} ${reason}`
			void options.persistRevert(field, entry).catch((error) => {
				options.log?.warn("geodb read source failed to persist auto-revert latch to ConfigMap", {
					field,
					reason,
					error: String(error),
				})
			})
		}
	}

	return {
		refreshIfDue,
		effectiveMode,
		configuredMode,
		isReverted,
		recordOutcome,
		reset: resetField,
	}
}
