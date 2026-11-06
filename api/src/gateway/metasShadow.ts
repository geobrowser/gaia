/**
 * The `metas` shadow: the indexer-cursor root (`metas { nodes { id cursor blockNumber } }`, a page's freshness sentinel).
 *
 * The rows are the pipeline's substreams cursors and block numbers, written to Postgres by the indexers. geo-db keeps
 * its own feeder position in its store and exposes no read of these rows, so there is nothing to compare: every
 * sampled operation is counted `unsupported_selection{no_geodb_home}` with its argument shape. That puts the root in
 * the coverage numbers (how much read traffic has no geo-db home) instead of leaving it invisible.
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `metas=shadow`; off by default. It never changes a response, reads
 * nothing from geo-db and logs counts only. Beside `entitiesConnection` the other shadow takes the operation and
 * counts the sentinel as `metas_cursor` (this shadow tolerates no other root).
 */

import type {FieldShadow} from "./gateway"
import {noteUnsupported} from "./lightShadow"
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported} from "./metrics"

export const METAS_FIELD = "metas"

type Deps = Parameters<typeof noteUnsupported>[0]

/** The `metas` shadow. Registers its metric series; never throws. */
export function createMetasShadow(deps: Deps): FieldShadow {
	activateGatewayMetrics(METAS_FIELD)
	return async ({params, document, classification}) => {
		try {
			recordGatewayShadow(METAS_FIELD, "unsupported_selection")
			recordGatewayUnsupported(METAS_FIELD, "no_geodb_home")
			noteUnsupported(
				deps,
				METAS_FIELD,
				"no_geodb_home",
				document,
				classification.operationName,
				params.variables,
			)
		} catch {}
	}
}
