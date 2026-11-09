/**
 * Configuration for the opt-in geo-db entity-card read (stage 1 of the Postgres -> geo-db cutover).
 *
 * GEODB_ENTITY_CARDS selects the mode:
 * - "off" (default, also any unrecognized value): the route is not mounted.
 * - "on": answer from geo-db; on any geo-db error or timeout fall back to Postgres for that request.
 * - "shadow": answer from Postgres, also ask geo-db, and log a structured diff when they differ.
 */

export type GeoDbCardMode = "off" | "on" | "shadow"

export type GeoDbCardConfig = {
	mode: GeoDbCardMode
	/** Path of the `geodb-client` binary. */
	clientBin: string
	/** Client key file (`--key`). */
	keyPath: string
	/** Binding file (`--binding`) naming the geo-db server's listener. */
	bindingPath: string
	/** JSON file mapping a gaia space id to its geo-db space id: `{"<space>": {"geo": "<id>"}}`. */
	spaceMapPath: string
	/** Per-request budget for the geo-db read, in milliseconds. */
	timeoutMs: number
	/** Long-lived `geodb-client pipe` workers, each one connection and one request at a time. */
	poolSize: number
	/**
	 * Read an entity's card across several Spaces in one `geodb-client pipe` line (`<space>,<space>\t<geoql>`).
	 * Needs a geodb-client built from geo-db 353ab26313 or later; an older one refuses the line and loses its
	 * worker, so this is off until the image is bumped. GEODB_MULTI_SPACE_READS=on.
	 */
	multiSpaceReads?: boolean
	/**
	 * Let the gateway's entity shadow read a card across every Space in the space map (one pipe line, needs
	 * `multiSpaceReads`) when PostGraphile's answer does not say where the entity lives, as `EntityExists` and
	 * `EntityTypes` do not. Off by default until its cost is measured on the live store. GEODB_EVERY_SPACE_READS=on.
	 */
	everySpaceReads?: boolean
	/**
	 * Also give the `entities` shadow the every-Space read (needs `everySpaceReads`): an `entities(filter: {id: {in}})`
	 * that selects no lists and no `spaceIds` (`EntityNames`) is compared on one sampled entity read across every
	 * Space. Off by default. GEODB_EVERY_SPACE_ENTITIES=on.
	 */
	everySpaceEntities?: boolean
	/**
	 * Let the gateway's entity shadow compare an entity's `backlinksList` against geo-db's card backlinks
	 * (`EntityBacklinksPage`): one read of the filter's Space, or, with `everySpaceReads`, of every Space when
	 * the filter names none. Off by default until its cost is measured on the live store. GEODB_BACKLINK_READS=on.
	 */
	backlinkReads?: boolean
	/**
	 * Let the `entities` shadow compare the EntitiesBatch selection (`entities(filter: {id: {in}}, spaceId) { .. allValuesList
	 * allRelationsList relationsList .. }`): its unfiltered `allRelationsList { spaceId }` projection is accepted (counted
	 * `membership_only`) instead of refusing the whole operation. Off by default. GEODB_ENTITIES_BATCH_SHADOW=on.
	 */
	entitiesBatch?: boolean
	/**
	 * Let the light `entities` shadow accept the vote scores PostGraphile computes from Postgres (`upvotes`, `downvotes`,
	 * `agrees`, `disagrees`, `commenters`, `commentScore`): geo-db holds none of them, so they are counted
	 * `derived_fields` and never compared; the rest of the selection compares as usual. Off by default.
	 * GEODB_DERIVED_FIELDS_SHADOW=on.
	 */
	derivedFields?: boolean
	/**
	 * Read an every-Space card in lines of at most MAX_LINE_SPACES Spaces when geo-db has loaded more than that, and
	 * concatenate the rows. Without it such a read is refused `many_spaces`. GEODB_EVERY_SPACE_CHUNKS=on.
	 */
	everySpaceChunks?: boolean
	/**
	 * Timeout in ms for an every-Space shadow read, which names 1579 Spaces and so can pass the pool's 500 ms
	 * (GEODB_EVERY_SPACE_TIMEOUT_MS). Unset: the pool's `timeoutMs`. Shadow reads only; serving is unchanged.
	 */
	everySpaceTimeoutMs?: number
	/**
	 * Let the light `entities` shadow read a single-key `orderBy: CREATED_AT_* | UPDATED_AT_*` page from geo-db (`.pick` and
	 * `.ties = .entity` on the order) and compare it with PostGraphile's. Needs a serve whose store has geo-db d3967dffbf: an
	 * older one refuses the members, so turn it on only after the serve pin passes that commit. Also needs
	 * `multiSpaceReads`, `everySpaceReads` and `everySpaceEntities`. Off by default. GEODB_ORDER_PICK_SHADOW=on.
	 */
	orderPick?: boolean
	/**
	 * Let the light `entities` shadow read an unordered `entities(spaceId(s), typeId | typeIds, first, offset)` page from geo-db
	 * as a type scan (`.args = .{ .types = .{..}, .first = offset + first }`, at most 4096) and compare the window's ids, in
	 * primary-key order, with PostGraphile's. Needs a serve whose store has geo-db c2f9d09cfa: an older one refuses the
	 * argument (counted `type_scan_refused`, never a failed request). Off by default. GEODB_TYPE_SCAN_SHADOW=on.
	 */
	typeScan?: boolean
	/**
	 * Let the `entitiesConnection` shadow lower an operation that asks only `totalCount` (`filter: {typeIds, relations: {some}}`) to
	 * geo-db's entities count form (`.args = .{ .type, .count = true, .where }`, DR-154 item 8) and compare the counts. Needs a serve
	 * whose store has the count form with `where`: an older one refuses it (counted `unsupported_selection` / `count_refused`, and
	 * costs one worker restart per refused read). A root with no Space argument is counted across every Space (a `*` line) and
	 * needs `GEODB_EVERY_SPACE_READS`. Off by default. GEODB_COUNT_ONLY_SHADOW=on.
	 */
	countOnly?: boolean
	/**
	 * Let the light `entities` shadow compare a relations list that is filtered by `typeId` and does not select the type
	 * (`relations(filter: {typeId: {in}}) { nodes { toEntity { id name } } }`): each row must be held under one of the
	 * filter's Types. Off by default. GEODB_RELATIONS_TYPE_FILTER_SHADOW=on.
	 */
	relationsTypeFilter?: boolean
	/**
	 * Compare the `valuesList` a relations list selects on each `toEntity` against geo-db's cards of those targets, read
	 * together (`.args.ids`; needs a serve of geo-db 8df213cbac or later). Off by default. GEODB_NESTED_TARGET_VALUES_SHADOW=on.
	 */
	nestedTargetValues?: boolean
	/**
	 * Settle a nested property, Type or target name geo-db prints as none (the thing is named in a Space outside the read)
	 * from its Name in every Space, one batch read. Off by default. GEODB_CROSS_SPACE_NAMES_SHADOW=on.
	 */
	crossSpaceNames?: boolean
	/**
	 * Settle an every-Space card read (an answer that names no Space) by asking geo-db which Spaces hold the entity
	 * (`.args.holders = true`) and reading the card in those alone, in place of the card across every loaded Space.
	 * Needs a serve whose store has geo-db 48739dfef5, so turn it on only after the serve pin passes that commit; a store
	 * that refuses the argument is detected and read the old way for a while. Also needs `everySpaceReads`. Off by
	 * default. GEODB_HOLDERS_LOOKUP=on.
	 */
	holdersLookup?: boolean
	/**
	 * Lifetime of the capability token a worker's handshake gets, in seconds: geo-db's `grant_ttl_s`
	 * (GEODB_GRANT_TTL_S on the serve, 300 by default). A worker is replaced before its token expires, so no
	 * read finds the token dead. Absent means the default.
	 */
	grantTtlS?: number
}

const DEFAULT_TIMEOUT_MS = 500
const DEFAULT_POOL_SIZE = 4
const MAX_POOL_SIZE = 64
const DEFAULT_GRANT_TTL_S = 300

export function parseMode(raw: string | undefined): GeoDbCardMode {
	const value = raw?.trim().toLowerCase()
	return value === "on" || value === "shadow" ? value : "off"
}

export function readGeoDbCardConfig(env: Record<string, string | undefined>): GeoDbCardConfig {
	const timeout = Number(env.GEODB_TIMEOUT_MS)
	const pool = Math.floor(Number(env.GEODB_POOL_SIZE))
	const grantTtl = Number(env.GEODB_GRANT_TTL_S)
	return {
		mode: parseMode(env.GEODB_ENTITY_CARDS),
		clientBin: env.GEODB_CLIENT_BIN ?? "geodb-client",
		keyPath: env.GEODB_KEY_PATH ?? "",
		bindingPath: env.GEODB_BINDING_PATH ?? "",
		spaceMapPath: env.GEODB_SPACE_MAP_PATH ?? "",
		timeoutMs: Number.isFinite(timeout) && timeout > 0 ? timeout : DEFAULT_TIMEOUT_MS,
		multiSpaceReads: env.GEODB_MULTI_SPACE_READS?.trim().toLowerCase() === "on",
		everySpaceReads: env.GEODB_EVERY_SPACE_READS?.trim().toLowerCase() === "on",
		everySpaceEntities: env.GEODB_EVERY_SPACE_ENTITIES?.trim().toLowerCase() === "on",
		backlinkReads: env.GEODB_BACKLINK_READS?.trim().toLowerCase() === "on",
		everySpaceChunks: env.GEODB_EVERY_SPACE_CHUNKS?.trim().toLowerCase() === "on",
		...(Number(env.GEODB_EVERY_SPACE_TIMEOUT_MS) > 0
			? {everySpaceTimeoutMs: Number(env.GEODB_EVERY_SPACE_TIMEOUT_MS)}
			: {}),
		entitiesBatch: env.GEODB_ENTITIES_BATCH_SHADOW?.trim().toLowerCase() === "on",
		derivedFields: env.GEODB_DERIVED_FIELDS_SHADOW?.trim().toLowerCase() === "on",
		orderPick: env.GEODB_ORDER_PICK_SHADOW?.trim().toLowerCase() === "on",
		typeScan: env.GEODB_TYPE_SCAN_SHADOW?.trim().toLowerCase() === "on",
		countOnly: env.GEODB_COUNT_ONLY_SHADOW?.trim().toLowerCase() === "on",
		relationsTypeFilter: env.GEODB_RELATIONS_TYPE_FILTER_SHADOW?.trim().toLowerCase() === "on",
		nestedTargetValues: env.GEODB_NESTED_TARGET_VALUES_SHADOW?.trim().toLowerCase() === "on",
		crossSpaceNames: env.GEODB_CROSS_SPACE_NAMES_SHADOW?.trim().toLowerCase() === "on",
		holdersLookup: env.GEODB_HOLDERS_LOOKUP?.trim().toLowerCase() === "on",
		poolSize: Number.isFinite(pool) && pool > 0 ? Math.min(pool, MAX_POOL_SIZE) : DEFAULT_POOL_SIZE,
		grantTtlS: Number.isFinite(grantTtl) && grantTtl > 0 ? grantTtl : DEFAULT_GRANT_TTL_S,
	}
}
