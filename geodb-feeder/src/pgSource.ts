/**
 * Read-only access to gaia's Postgres (the kg-indexer's tables) for the backfill and the replay driver.
 *
 * One connection, one statement at a time. Every query runs in its own `BEGIN READ ONLY` transaction with a
 * `SET LOCAL statement_timeout`, which is safe behind pgbouncer's transaction pooling and makes a write
 * impossible even if a query were wrong.
 */
import pg from "pg"
import type {PgRelationRow, PgValueRow} from "./pgAdapter.js"

export type Queryable = {query<R>(text: string, params?: unknown[]): Promise<{rows: R[]}>}

/**
 * A pg client config from `DATABASE_URL`. With `ca` (the cluster CA, `DATABASE_SSL_CA`) the connection is
 * verified against it; `sslmode` in the URL is dropped so it cannot override that.
 */
export function pgConfig(url: string, ca: string | undefined): pg.ClientConfig {
	if (!ca) return {connectionString: url}
	const u = new URL(url)
	u.searchParams.delete("sslmode")
	return {connectionString: u.toString(), ssl: {ca, rejectUnauthorized: true}}
}

export class ReadOnlyPg {
	private client: pg.Client | null = null

	constructor(
		private readonly config: pg.ClientConfig,
		private readonly timeoutMs = 60_000,
	) {}

	private async connect(): Promise<pg.Client> {
		if (this.client) return this.client
		const c = new pg.Client(this.config)
		c.on("error", () => {
			if (this.client === c) this.client = null
		})
		await c.connect()
		this.client = c
		return c
	}

	async read<R>(text: string, params: unknown[] = [], timeoutMs = this.timeoutMs): Promise<R[]> {
		const c = await this.connect()
		try {
			await c.query("BEGIN READ ONLY")
			await c.query(`SET LOCAL statement_timeout = ${Math.max(1000, Math.floor(timeoutMs))}`)
			const r = await c.query(text, params)
			await c.query("COMMIT")
			return r.rows as R[]
		} catch (e) {
			await c.query("ROLLBACK").catch(() => {})
			if (this.client === c) {
				this.client = null
				await c.end().catch(() => {})
			}
			throw e
		}
	}

	async close(): Promise<void> {
		const c = this.client
		this.client = null
		await c?.end().catch(() => {})
	}
}

/** The backfill's view of Postgres. Ids go in and come out as Postgres prints uuids (dashed). */
export type BackfillSource = {
	/** Rows per space (values + relations), for ordering Spaces largest first. */
	spaceSizes(): Promise<Map<string, number>>
	/** Every entity with a value or an outgoing relation in `space`. */
	listEntities(space: string): Promise<string[]>
	readBatch(space: string, entities: string[]): Promise<{values: PgValueRow[]; relations: PgRelationRow[]}>
}

const dashed = (id: string) =>
	id.includes("-") ? id : `${id.slice(0, 8)}-${id.slice(8, 12)}-${id.slice(12, 16)}-${id.slice(16, 20)}-${id.slice(20)}`

export function pgBackfillSource(db: ReadOnlyPg): BackfillSource {
	return {
		async spaceSizes() {
			// Two index-only scans of the space_id indexes; run once per backfill.
			const rows = await db.read<{space_id: string; n: string}>(
				`SELECT space_id::text AS space_id, sum(n)::text AS n FROM (
					SELECT space_id, count(*) AS n FROM "values" GROUP BY space_id
					UNION ALL
					SELECT space_id, count(*) AS n FROM relations GROUP BY space_id
				) t GROUP BY space_id`,
				[],
				300_000,
			)
			return new Map(rows.map((r) => [r.space_id.replaceAll("-", ""), Number(r.n)]))
		},
		async listEntities(space) {
			const rows = await db.read<{id: string}>(
				`SELECT entity_id::text AS id FROM "values" WHERE space_id = $1::uuid
				 UNION
				 SELECT from_entity_id::text FROM relations WHERE space_id = $1::uuid`,
				[dashed(space)],
				300_000,
			)
			return rows.map((r) => r.id).sort()
		},
		async readBatch(space, entities) {
			const ids = entities.map(dashed)
			const values = await db.read<PgValueRow>(
				`SELECT entity_id::text AS entity_id, property_id::text AS property_id, text, language,
				        integer::text AS integer, float::text AS float, decimal::text AS decimal, boolean,
				        datetime, date, point
				 FROM "values" WHERE space_id = $1::uuid AND entity_id = ANY($2::uuid[])`,
				[dashed(space), ids],
			)
			const relations = await db.read<PgRelationRow>(
				`SELECT id::text AS id, from_entity_id::text AS from_entity_id, to_entity_id::text AS to_entity_id,
				        type_id::text AS type_id
				 FROM relations WHERE space_id = $1::uuid AND from_entity_id = ANY($2::uuid[])`,
				[dashed(space), ids],
			)
			return {values, relations}
		},
	}
}
