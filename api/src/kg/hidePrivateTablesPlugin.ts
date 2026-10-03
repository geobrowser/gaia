import {makeJSONPgSmartTagsPlugin} from "graphile-utils"

// Hides tables that are service-internal state, not graph data, from the GraphQL schema.
//
// PostGraphile publishes every table the API role can SELECT unless it is told not to, so a
// table added for a backend service lands in the public API by default. The notification
// service's tables hold delivery state and webhook configuration, none of which is graph data.
//
// Omitting a class removes its root fields, its `node(nodeId)` resolution and its relation
// fields. The services that own these tables read them directly from Postgres, never through
// this API.
export default makeJSONPgSmartTagsPlugin({
	version: 1,
	config: {
		class: {
			"public.app_webhooks": {tags: {omit: true}},
			"public.notification_outbox": {tags: {omit: true}},
			"public.notification_deliveries": {tags: {omit: true}},
			"public.notification_poll_cursors": {tags: {omit: true}},
		},
	},
})
