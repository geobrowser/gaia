import {makeJSONPgSmartTagsPlugin} from "graphile-utils"

// Hides Postgres functions from the GraphQL schema without dropping them.
export default makeJSONPgSmartTagsPlugin({
	version: 1,
	config: {
		procedure: {
			"public.entities_ordered_by_score": {tags: {omit: true}},
		},
	},
})
