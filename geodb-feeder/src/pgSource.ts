/**
 * Read-only access to gaia's Postgres (the kg-indexer's tables) for the backfill and the replay driver.
 *
 * One connection, one statement at a time. Every query runs in its own `BEGIN READ ONLY` transaction with a
 * `SET LOCAL statement_timeout`, which is safe behind pgbouncer's transaction pooling and makes a write
 * impossible even if a query were wrong.
 */
import pg from "pg"
import type {PgRelationRow, PgValueRow} from "./pgAdapter.js"
import type {ProposalActionRow} from "./proposalActionsKeyed.js"
import type {EntityRankingConfigRow} from "./entityRankingConfigKeyed.js"
import type {InterestConfigRow, InterestSignalWeightRow} from "./interestConfigKeyed.js"
import type {ForYouConfigRow} from "./forYouConfigKeyed.js"
import type {SpaceVotingSettingsRow} from "./spaceVotingSettingsKeyed.js"
import type {ProposalVersionRow} from "./proposalVersionsKeyed.js"
import type {ProposalStatusRow} from "./proposalStatusKeyed.js"
import type {RankingBlockRow, RankingItemRow, RankingRow} from "./ranksKeyed.js"
import type {ProposalVoteRow} from "./proposalVotesKeyed.js"
import type {SubspaceRow} from "./subspacesKeyed.js"
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

/** Every `proposal_votes` row, keyset-paged on its primary key (proposal_id, proposal_version, voter_id). */
export async function* readProposalVoteRows(db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<ProposalVoteRow> {
	let after: ProposalVoteRow | null = null
	for (;;) {
		const page: ProposalVoteRow[] = await db.read<ProposalVoteRow>(
			`SELECT proposal_id::text AS proposal_id, proposal_version::int AS proposal_version, voter_id::text AS voter_id,
			        space_id::text AS space_id, vote::text AS vote, created_at::text AS created_at,
			        created_at_block::text AS created_at_block
			 FROM proposal_votes
			 WHERE $1::boolean OR (proposal_votes.proposal_id, proposal_votes.proposal_version, proposal_votes.voter_id)
			       > ($2::uuid, $3::int, $4::uuid)
			 -- Qualified, as in readUserVoteRows: a bare name would bind to the ::text output alias.
			 ORDER BY proposal_votes.proposal_id, proposal_votes.proposal_version, proposal_votes.voter_id
			 LIMIT $5`,
			after === null ? [true, null, null, null, pageSize] : [false, after.proposal_id, after.proposal_version, after.voter_id, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		after = page[page.length - 1] as ProposalVoteRow
	}
}

/**
 * Every `proposal_actions` row, with the block of its version (`proposal_versions.version_created_at_block`), keyset-paged
 * on the primary key (proposal_id, proposal_version, index). The version ordinal is not returned: the record is keyed by
 * the block (proposalActionsKeyed.ts).
 */
export async function* readProposalActionRows(db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<ProposalActionRow & {proposal_version: number}> {
	let after: (ProposalActionRow & {proposal_version: number}) | null = null
	for (;;) {
		const page: (ProposalActionRow & {proposal_version: number})[] = await db.read<ProposalActionRow & {proposal_version: number}>(
			`SELECT a.proposal_id::text AS proposal_id, a.proposal_version::int AS proposal_version, a."index"::int AS "index",
			        v.version_created_at_block::text AS block, a.action_type::text AS action_type, a.target_id::text AS target_id,
			        a.content_uri AS content_uri, encode(a.metadata, 'hex') AS metadata, encode(a.content_id, 'hex') AS content_id,
			        a.quorum::text AS quorum, a.duration::text AS duration,
			        a.partial_percentage_support_threshold::text AS partial_percentage_support_threshold,
			        a.universal_percentage_support_threshold::text AS universal_percentage_support_threshold,
			        a.flat_support_threshold::text AS flat_support_threshold,
			        a.disable_fast_path_access_for_new_members AS disable_fast_path_access_for_new_members,
			        a.execution_grace_period::text AS execution_grace_period
			 FROM proposal_actions a
			 JOIN proposal_versions v ON v.proposal_id = a.proposal_id AND v.proposal_version = a.proposal_version
			 WHERE $1::boolean OR (a.proposal_id, a.proposal_version, a."index") > ($2::uuid, $3::int, $4::int)
			 ORDER BY a.proposal_id, a.proposal_version, a."index"
			 LIMIT $5`,
			after === null ? [true, null, null, null, pageSize] : [false, after.proposal_id, after.proposal_version, after.index, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		after = page[page.length - 1] as ProposalActionRow & {proposal_version: number}
	}
}

/** Every `subspaces` row (type verified/related) and `subspace_topics` row (type topic), for the subspaces backfill. Small tables: one read. */
export async function readSubspaceRows(db: ReadOnlyPg): Promise<SubspaceRow[]> {
	return db.read<SubspaceRow>(
		`SELECT parent_space_id::text AS parent_space_id, child_space_id::text AS target_id, type::text AS type FROM subspaces
		 UNION ALL
		 SELECT space_id::text, topic_id::text, 'topic' FROM subspace_topics`,
		[],
		300_000,
	)
}

/** The one `personalization.for_you_config` row. Numbers come back as text. */
export async function* readForYouConfigRows(db: ReadOnlyPg): AsyncGenerator<ForYouConfigRow> {
	yield* await db.read<ForYouConfigRow>(
		`SELECT max_boost_days::text AS max_boost_days, interest_half_saturation::text AS interest_half_saturation,
		        exploration_share::text AS exploration_share, max_topics::text AS max_topics
		 FROM personalization.for_you_config`,
		[],
		60_000,
	)
}

/** The one `entity_ranking_config` row (`WHERE id`), every tunable as text. `updated_at` is not mirrored. */
export async function* readEntityRankingConfigRows(db: ReadOnlyPg): AsyncGenerator<EntityRankingConfigRow> {
	yield* await db.read<EntityRankingConfigRow>(
		`SELECT tau_seconds::text AS tau_seconds,
		        wilson_z::text AS wilson_z,
		        prior_positive::text AS prior_positive,
		        prior_negative::text AS prior_negative,
		        quality_floor::text AS quality_floor,
		        intrinsic_cap::text AS intrinsic_cap,
		        intrinsic_property_target::text AS intrinsic_property_target,
		        intrinsic_relation_target::text AS intrinsic_relation_target,
		        participation_weight::text AS participation_weight,
		        participation_cap::text AS participation_cap,
		        comment_weight::text AS comment_weight,
		        comment_cap::text AS comment_cap,
		        topic_interested_weight::text AS topic_interested_weight,
		        topic_debate_weight::text AS topic_debate_weight,
		        topic_cap::text AS topic_cap,
		        watch_window_days::text AS watch_window_days,
		        watch_prior_impressions::text AS watch_prior_impressions,
		        watch_reference_seconds::text AS watch_reference_seconds,
		        watch_ratio_weight::text AS watch_ratio_weight,
		        watch_completed_weight::text AS watch_completed_weight,
		        watch_plays_weight::text AS watch_plays_weight,
		        watch_claim_votes_weight::text AS watch_claim_votes_weight,
		        watch_cap::text AS watch_cap,
		        watch_fresh_boost::text AS watch_fresh_boost,
		        watch_fresh_impressions::text AS watch_fresh_impressions,
		        trending_boost::text AS trending_boost,
		        trending_topic_boost::text AS trending_topic_boost,
		        trending_day_factor::text AS trending_day_factor,
		        trending_debate_factor::text AS trending_debate_factor,
		        trending_debate_window_days::text AS trending_debate_window_days,
		        trending_horizon_days::text AS trending_horizon_days
		 FROM public.entity_ranking_config WHERE id`,
		[],
		60_000,
	)
}

/** The one `personalization.interest_config` row (`WHERE id`), every tunable as text; `sweep_lookback` in seconds. */
export async function* readInterestConfigRows(db: ReadOnlyPg): AsyncGenerator<InterestConfigRow> {
	yield* await db.read<InterestConfigRow>(
		`SELECT half_life_days::text AS half_life_days,
		        spread_fraction::text AS spread_fraction,
		        spread_max_neighbours::text AS spread_max_neighbours,
		        cooccurrence_min_support::text AS cooccurrence_min_support,
		        cooccurrence_max_topics_per_entity::text AS cooccurrence_max_topics_per_entity,
		        extract(epoch FROM sweep_lookback)::text AS sweep_lookback_seconds
		 FROM personalization.interest_config WHERE id`,
		[],
		60_000,
	)
}

/** Every `personalization.interest_signal_weights` row (a handful of kinds), `weight` as text. */
export async function* readInterestSignalWeightRows(db: ReadOnlyPg): AsyncGenerator<InterestSignalWeightRow> {
	yield* await db.read<InterestSignalWeightRow>(
		`SELECT kind, weight::text AS weight, decays, note
		 FROM personalization.interest_signal_weights ORDER BY kind`,
		[],
		60_000,
	)
}

/** Every `space_voting_settings` row (one per Space, a small table), read in one query. The bigints and `updated_at*` come back as text. */
export async function* readSpaceVotingSettingsRows(db: ReadOnlyPg): AsyncGenerator<SpaceVotingSettingsRow> {
	yield* await db.read<SpaceVotingSettingsRow>(
		`SELECT space_id::text AS space_id,
		        partial_percentage_support_threshold::text AS partial_percentage_support_threshold,
		        universal_percentage_support_threshold::text AS universal_percentage_support_threshold,
		        flat_support_threshold::text AS flat_support_threshold, quorum::text AS quorum, duration::text AS duration,
		        disable_fast_path_access_for_new_members AS disable_fast_path_access_for_new_members,
		        execution_grace_period::text AS execution_grace_period, updated_at::text AS updated_at,
		        updated_at_block::text AS updated_at_block
		 FROM space_voting_settings
		 ORDER BY space_id`,
		[],
		300_000,
	)
}

/**
 * Every `proposal_versions` row, keyset-paged on the primary key (proposal_id, proposal_version). The ordinal is
 * returned for the paging only: the record is keyed by the block (proposalVersionsKeyed.ts). `threshold` and the
 * tally counts are not selected.
 */
export async function* readProposalVersionRows(db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<ProposalVersionRow & {proposal_version: number}> {
	let after: (ProposalVersionRow & {proposal_version: number}) | null = null
	for (;;) {
		const page: (ProposalVersionRow & {proposal_version: number})[] = await db.read<ProposalVersionRow & {proposal_version: number}>(
			`SELECT v.proposal_id::text AS proposal_id, v.proposal_version::int AS proposal_version,
			        v.version_created_at_block AS block, v.voting_mode::text AS voting_mode,
			        v.start_time::text AS start_time, v.end_time::text AS end_time, v.quorum::text AS quorum,
			        v.partial_percentage_support_threshold::text AS partial_percentage_support_threshold,
			        v.universal_percentage_support_threshold::text AS universal_percentage_support_threshold,
			        v.flat_support_threshold::text AS flat_support_threshold,
			        v.execute_by::text AS execute_by, v.name AS name, v.version_created_at AS version_created_at
			 FROM proposal_versions v
			 WHERE $1::boolean OR (v.proposal_id, v.proposal_version) > ($2::uuid, $3::int)
			 ORDER BY v.proposal_id, v.proposal_version
			 LIMIT $4`,
			after === null ? [true, null, null, pageSize] : [false, after.proposal_id, after.proposal_version, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		after = page[page.length - 1] as ProposalVersionRow & {proposal_version: number}
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

/**
 * Every `proposals` row with `executed_at` or `unexecutable_at` set (the stored inputs of a proposal's status; the
 * status itself is computed on read), keyset-paged on the primary key. Both come back as text.
 */
export async function* readProposalStatusRows(db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<ProposalStatusRow> {
	let after: string | null = null
	for (;;) {
		const page: ProposalStatusRow[] = await db.read<ProposalStatusRow>(
			`SELECT id::text AS id, executed_at::text AS executed_at, unexecutable_at::text AS unexecutable_at
			 FROM proposals
			 WHERE (executed_at IS NOT NULL OR unexecutable_at IS NOT NULL) AND ($1::uuid IS NULL OR id > $1::uuid)
			 ORDER BY id
			 LIMIT $2`,
			[after, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		after = (page[page.length - 1] as ProposalStatusRow).id
	}
}

/** The microseconds since the epoch of a timestamptz column, as text (null stays null). */
const micros = (col: string) => `(extract(epoch from ${col})::numeric * 1000000)::bigint::text`

/** Keyset paging over a table's all-uuid primary key; `keys` are the key columns in order, `select` aliases them by name. */
async function* pageByKey<R extends Record<string, unknown>>(
	db: ReadOnlyPg,
	select: string,
	table: string,
	keys: string[],
	pageSize: number,
): AsyncGenerator<R> {
	let after: string[] | null = null
	const tuple = keys.map((k) => `t.${k}`).join(", ")
	const cast = keys.map((_, i) => `$${i + 2}::uuid`).join(", ")
	for (;;) {
		const page: R[] = await db.read<R>(
			`SELECT ${select} FROM ${table} t WHERE $1::boolean OR (${tuple}) > (${cast}) ORDER BY ${tuple} LIMIT $${keys.length + 2}`,
			after === null ? [true, ...keys.map(() => null), pageSize] : [false, ...after, pageSize],
			300_000,
		)
		yield* page
		if (page.length < pageSize) return
		const last = page[page.length - 1] as R
		after = keys.map((k) => last[k] as string)
	}
}

/** Every `ranks.ranking_blocks` row (keyset-paged on (id, space_id)), dates as microseconds. */
export const readRankingBlockRows = (db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<RankingBlockRow> =>
	pageByKey<RankingBlockRow>(
		db,
		`t.id::text AS id, t.space_id::text AS space_id, t.name AS name, t.filter AS filter,
		 ${micros("t.start_date")} AS start_date, ${micros("t.end_date")} AS end_date,
		 t.restriction_id::text AS restriction_id, t.ranking_type::text AS ranking_type, t.submission_frequency AS submission_frequency`,
		"ranks.ranking_blocks",
		["id", "space_id"],
		pageSize,
	)

/** Every `ranks.rankings` row (keyset-paged on (id, space_id)), `submitted_at` as microseconds. */
export const readRankingRows = (db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<RankingRow> =>
	pageByKey<RankingRow>(
		db,
		`t.id::text AS id, t.space_id::text AS space_id, t.block_id::text AS block_id, t.author_address AS author_address,
		 t.rank_type AS rank_type, ${micros("t.submitted_at")} AS submitted_at,
		 t.updated_at_block::text AS updated_at_block, t.update_index::text AS update_index`,
		"ranks.rankings",
		["id", "space_id"],
		pageSize,
	)

/** Every `ranks.ranking_items` row (keyset-paged on its primary key). */
export const readRankingItemRows = (db: ReadOnlyPg, pageSize = 5000): AsyncGenerator<RankingItemRow> =>
	pageByKey<RankingItemRow>(
		db,
		`t.ranking_id::text AS ranking_id, t.entity_id::text AS entity_id, t.space_id::text AS space_id,
		 t.position AS position, t.weight AS weight`,
		"ranks.ranking_items",
		["ranking_id", "entity_id", "space_id"],
		pageSize,
	)
