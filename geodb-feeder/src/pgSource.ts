/**
 * Read-only access to gaia's Postgres (the kg-indexer's tables) for the backfill and the replay driver.
 *
 * One connection, one statement at a time. Every query runs in its own `BEGIN READ ONLY` transaction with a
 * `SET LOCAL statement_timeout`, which is safe behind pgbouncer's transaction pooling and makes a write
 * impossible even if a query were wrong.
 */
import pg from "pg"
import type {PgRelationRow, PgValueRow} from "./pgAdapter.js"
import type {UserVoteRow} from "./votesKeyed.js"

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
				        type_id::text AS type_id, position
				 FROM relations WHERE space_id = $1::uuid AND from_entity_id = ANY($2::uuid[])`,
				[dashed(space), ids],
			)
			return {values, relations}
		},
	}
}

export type MembershipRole = "member" | "editor"
export type MembershipRow = {space_id: string; member_space_id: string; role: MembershipRole}

/** Every current member and editor row (the kg-indexer's `members` and `editors`), for the membership backfill. */
export async function readMembershipRows(db: ReadOnlyPg): Promise<MembershipRow[]> {
	return db.read<MembershipRow>(
		`SELECT space_id::text AS space_id, member_space_id::text AS member_space_id, 'member' AS role FROM members
		 UNION ALL
		 SELECT space_id::text, member_space_id::text, 'editor' FROM editors`,
		[],
		300_000,
	)
}

/** Member ids (dashes stripped) per Space and role, for the shadow comparison's id-set compare. */
export async function readMembershipIds(db: ReadOnlyPg): Promise<Map<string, Record<MembershipRole, string[]>>> {
	const rows = await readMembershipRows(db)
	const out = new Map<string, Record<MembershipRole, string[]>>()
	for (const r of rows) {
		const key = r.space_id.replaceAll("-", "").toLowerCase()
		const rec = out.get(key) ?? {member: [], editor: []}
		rec[r.role].push(r.member_space_id.replaceAll("-", "").toLowerCase())
		out.set(key, rec)
	}
	return out
}

/**
 * Every `user_votes` row, in primary-key order, a page at a time (keyset pagination on the primary key, so each page
 * is an index range scan and no page is held open across geo-db writes). For the votes backfill.
 */
export async function* readUserVoteRows(db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<UserVoteRow> {
	let after: UserVoteRow | null = null
	for (;;) {
		const page: UserVoteRow[] = await db.read<UserVoteRow>(
			`SELECT user_id::text AS user_id, object_id::text AS object_id, object_type::int AS object_type,
			        space_id::text AS space_id, vote_kind::int AS vote_kind, vote_type::int AS vote_type,
			        floor(extract(epoch FROM voted_at))::bigint::text AS voted_at_seconds
			 FROM user_votes
			 WHERE $1::boolean OR (user_votes.user_id, user_votes.object_id, user_votes.object_type, user_votes.space_id, user_votes.vote_kind)
			       > ($2::uuid, $3::uuid, $4::smallint, $5::uuid, $6::smallint)
			 -- Qualified: a bare name here would bind to the ::text output alias, sorting by text while the keyset
			 -- WHERE compares uuids; under a collation where the two orders differ, paging skips rows (pgSource.votes test).
			 ORDER BY user_votes.user_id, user_votes.object_id, user_votes.object_type, user_votes.space_id, user_votes.vote_kind
			 LIMIT $7`,
			after === null
				? [true, null, null, null, null, null, pageSize]
				: [false, after.user_id, after.object_id, after.object_type, after.space_id, after.vote_kind, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		after = page[page.length - 1] as UserVoteRow
	}
}

export type EntityTimesRow = {id: string; created_at: string | null; updated_at: string | null}

/** `entities.created_at` / `updated_at` (text, unix seconds) for the given entities, for the times backfill. */
export async function readEntityTimes(db: ReadOnlyPg, entities: string[]): Promise<EntityTimesRow[]> {
	return db.read<EntityTimesRow>(
		`SELECT id::text AS id, created_at::text AS created_at, updated_at::text AS updated_at
		 FROM entities WHERE id = ANY($1::uuid[])`,
		[entities.map(dashed)],
	)
}

/** The times backfill's view of Postgres: the same entity listing the backfill uses, plus the times. */
export type TimesSource = Pick<BackfillSource, "listEntities"> & {
	readTimes(entities: string[]): Promise<EntityTimesRow[]>
}

export const pgTimesSource = (db: ReadOnlyPg): TimesSource => ({
	listEntities: pgBackfillSource(db).listEntities,
	readTimes: (entities) => readEntityTimes(db, entities),
})
