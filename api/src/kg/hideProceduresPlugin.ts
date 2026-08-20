import {makeJSONPgSmartTagsPlugin} from "graphile-utils"

// Hides Postgres functions from the GraphQL schema without dropping them.
export default makeJSONPgSmartTagsPlugin({
	version: 1,
	config: {
		procedure: {
			"public.entities_ordered_by_score": {tags: {omit: true}},

			// Explore feed (Phase A) internals. These are computation helpers and a
			// write path, not API surface.
			//
			// entity_ranking_score MUST be hidden for a second reason: it inflects to
			// the same GraphQL key as the entity_ranking_scores table
			// ("entityRankingScore"), and the collision fails the whole schema build.
			//
			// refresh_entity_ranking_scores is a write path — exposing it would let any
			// caller trigger recomputation over arbitrary entity id arrays.
			//
			// entities_ranked_for_feed is deliberately NOT hidden: it is the feed's
			// entry point, and the exclusions it applies are why callers should use it
			// rather than a bare orderBy.
			"public.wilson_lower_bound": {tags: {omit: true}},
			"public.entity_intrinsic_score": {tags: {omit: true}},
			"public.entity_participation_score": {tags: {omit: true}},
			"public.entity_ranking_score": {tags: {omit: true}},
			"public.refresh_entity_ranking_scores": {tags: {omit: true}},
		},
	},
})
