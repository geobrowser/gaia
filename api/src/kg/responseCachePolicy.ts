/**
 * What the GraphQL response cache is allowed to store.
 *
 * Lives in its own module so it can be unit tested: importing `postgraphile.ts`
 * opens a Postgres pool and builds the PostGraphile schema at module load.
 */

/**
 * True when a GraphQL result carries at least one root field with actual data.
 *
 * Used to keep "not there yet" responses out of the response cache. Writes in
 * this system land out of band — on-chain, then indexer, then Postgres — so no
 * GraphQL mutation ever runs and the cache's mutation-driven invalidation never
 * fires. A cached empty result therefore survives its full TTL no matter what
 * happens in the database.
 *
 * That is what broke in #655: a client polling `Query.spaces` for a freshly
 * created space cached the pre-indexing empty response for 60s and kept being
 * served it, leaving the "Create Personal Space" modal stuck for a full minute.
 * The response cache was disabled outright rather than narrowed.
 *
 * Non-empty results stay cacheable — by then the write has landed, which is the
 * expensive-and-stable case the cache exists for (the spaces and entity lists
 * run 2-15 MB).
 *
 * Bounded on purpose: this fixes empty -> non-empty, not a populated list that
 * is missing a newly added member. Closing that needs real invalidation driven
 * by the indexer.
 */
export function hasCacheableData(data: unknown): boolean {
	if (data === null || data === undefined) return false
	if (typeof data !== "object") return true
	if (Array.isArray(data)) return data.length > 0

	const record = data as Record<string, unknown>

	// Relay-style connection: trust the collection, not the wrapper object.
	if (Array.isArray(record.nodes)) return record.nodes.length > 0
	if (Array.isArray(record.edges)) return record.edges.length > 0
	if (typeof record.totalCount === "number") return record.totalCount > 0

	// Root payload (or a plain object): non-empty if any field carries data.
	// `{entity: null}` and `{spaces: []}` are both misses; `{entity: {...}}` is a hit.
	const values = Object.values(record)
	if (values.length === 0) return false
	return values.some((value) => hasCacheableData(value))
}
