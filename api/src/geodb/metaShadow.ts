/**
 * The `meta` table shadow: the indexer cursors (`meta`: id, cursor, block_number; two rows live) against geo-db's derived
 * collection 10 (`cursors`), sampled on a timer instead of per request.
 *
 * Why a timer. The gateway's `metas` shadow (gateway/metasShadow.ts) only runs when a client sends a `metas` operation as
 * its single root. Clients ask for `metas` only as a freshness sentinel beside `entitiesConnection`, which the other shadow
 * tolerates and counts uncompared, so the field had no series at all in seven days. This sampler reads the same Postgres
 * rows off the request path, builds the response shape the `metas` root returns, and hands it to the same comparison
 * (`runMetas`), counting into the same counter the scorecard reads: `gaia_api_gateway_shadow_total{field="metas"}` and its
 * unsupported and uncompared companions. Postgres stays the answer; nothing is served from geo-db.
 *
 * Load: one SELECT over a two-row table and at most MAX_SAMPLED_ROWS derived `get`s per pass, every
 * GEODB_META_SHADOW_INTERVAL_MS (default 5 minutes). That is the whole cost. GEODB_META_SHADOW defaults to on (shadow);
 * only an explicit 0 / off / false turns it off.
 *
 * A Postgres failure is logged and counts nothing (the series says what geo-db did, not what Postgres did). Logs counts
 * only, never an id, cursor or block.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {MAX_SAMPLED_ROWS, METAS_FIELD, type MetasPlan, runMetas} from "../gateway/metasShadow"
import {
	activateGatewayMetrics,
	recordGatewayShadow,
	recordGatewayUncompared,
	recordGatewayUnsupported,
} from "../gateway/metrics"
import type {DerivedReader} from "./derived"
import {createIntervalSampler, DEFAULT_SAMPLER_INTERVAL_MS, type IntervalSampler} from "./intervalSampler"

export type MetaShadowConfig = {enabled: boolean; intervalMs: number}

/** Default ON; `GEODB_META_SHADOW` of 0, off or false disables it. */
export function readMetaShadowConfig(env: Record<string, string | undefined>): MetaShadowConfig {
	const flag = env.GEODB_META_SHADOW?.trim().toLowerCase()
	const n = Number(env.GEODB_META_SHADOW_INTERVAL_MS)
	return {
		enabled: !(flag === "0" || flag === "off" || flag === "false"),
		intervalMs: Number.isInteger(n) && n >= 1 ? Math.min(n, 86_400_000) : DEFAULT_SAMPLER_INTERVAL_MS,
	}
}

const PLAN: MetasPlan = {
	key: METAS_FIELD,
	nodesKey: "nodes",
	idKey: "id",
	cursorKey: "cursor",
	blockKey: "blockNumber",
	uncompared: [],
}

/** Postgres' `meta` rows, shaped as the body of `metas { nodes { id cursor blockNumber } }`. */
export async function readPostgresMetas(db: Database): Promise<unknown> {
	const result = await db.execute(
		sql`SELECT id, cursor, block_number FROM public.meta ORDER BY id LIMIT ${MAX_SAMPLED_ROWS + 1}`,
	)
	const rows = (result as {rows?: Record<string, unknown>[]}).rows ?? []
	return {
		data: {
			[METAS_FIELD]: {
				nodes: rows.map((r) => ({id: r.id, cursor: r.cursor, blockNumber: r.block_number})),
			},
		},
	}
}

export type MetaShadow = IntervalSampler

export function createMetaShadow(deps: {
	config: MetaShadowConfig
	db: Database
	reader: Pick<DerivedReader, "get">
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	readBody?: (db: Database) => Promise<unknown>
}): MetaShadow {
	const {config, db, reader, log} = deps
	const readBody = deps.readBody ?? readPostgresMetas
	activateGatewayMetrics(METAS_FIELD)

	async function pass(): Promise<void> {
		let body: unknown
		try {
			body = await readBody(db)
		} catch (error) {
			log.warn("meta shadow: the Postgres read failed", {error: String(error)})
			return
		}
		const outcome = await runMetas(reader as DerivedReader, PLAN, body)
		if (outcome.result === "skip") return
		if (outcome.result === "failed") return void recordGatewayShadow(METAS_FIELD, "failed")
		if (outcome.result === "unsupported") {
			recordGatewayShadow(METAS_FIELD, "unsupported_selection")
			recordGatewayUnsupported(METAS_FIELD, outcome.reason)
			return
		}
		for (const reason of new Set(outcome.uncompared)) recordGatewayUncompared(METAS_FIELD, reason)
		recordGatewayShadow(METAS_FIELD, outcome.result)
		if (outcome.result === "mismatch") log.warn("meta shadow mismatch", outcome.detail)
	}

	return createIntervalSampler({
		enabled: config.enabled,
		intervalMs: config.intervalMs,
		pass,
		log,
		label: "meta shadow",
	})
}
