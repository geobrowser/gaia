import {makeJSONPgSmartTagsPlugin} from "graphile-utils"

/**
 * Scopes `@graphile/pg-aggregates` to the tables that actually need faceted counts (GEO-2796).
 *
 * The plugin adds `aggregates` / `groupedAggregates` to every connection by default — all 45 of
 * them. Most are pointless surface and some would be expensive (a grouped `COUNT(DISTINCT ...)`
 * over a large table is not free), so we run it opt-in: `disableAggregatesByDefault: true` in
 * graphileBuildOptions turns it off everywhere, and these `aggregates: "on"` tags turn it back on
 * only where a facet menu reads it.
 *
 * - relations: the facet counts themselves — group by TO_ENTITY_ID (topic) or SPACE_ID and
 *   distinctCount fromEntityId, filtered via fromEntity. This is the whole point of the ticket.
 * - entities: the primary corpus; totalCount/aggregates over it are generally useful and indexed.
 *
 * Add a table here (rather than lifting the default) only when a specific facet needs it, so the
 * expensive-aggregate surface stays deliberate.
 */
export default makeJSONPgSmartTagsPlugin({
	version: 1,
	config: {
		class: {
			"public.relations": {tags: {aggregates: "on"}},
			"public.entities": {tags: {aggregates: "on"}},
		},
	},
})
