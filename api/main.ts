import {swaggerUI} from "@hono/swagger-ui"
import {Hono} from "hono"
import {cors} from "hono/cors"
import {openAPISpecs} from "hono-openapi"
import {createForYouConfigShadow, readForYouConfigShadowConfig} from "./src/forYou/configShadow"
import {
	createEntityRankingConfigShadow,
	readEntityRankingConfigShadowConfig,
} from "./src/forYou/entityRankingConfigShadow"
import {createFeedExperimentShadow, readFeedExperimentShadowConfig} from "./src/forYou/feedExperimentShadow"
import {MIN_INTERNAL_TOKEN_LENGTH, mountInternalRoutes} from "./src/forYou/router"
import {readGatewayConfig} from "./src/gateway/config"
import {
	createEntitiesConnectionShadow,
	createEntitiesOrderedShadow,
	ENTITIES_CONNECTION_FIELD,
	ENTITIES_ORDERED_FIELD,
} from "./src/gateway/entitiesConnectionShadow"
import {createEntityResolver} from "./src/gateway/entityServe"
import {createEntityShadow, createHoldersGate, ENTITY_FIELD} from "./src/gateway/entityShadow"
import {createGateway, fieldModeOf, type GatewayDeps} from "./src/gateway/gateway"
import {createMetasShadow, METAS_FIELD} from "./src/gateway/metasShadow"
import {createPointReadShadows, POINT_READ_FIELDS} from "./src/gateway/pointReadShadows"
import {createGatewayProbe, readGatewayProbeConfig} from "./src/gateway/probe"
import {createRankTypedObserver, readRankTypedConfig} from "./src/gateway/rankTypedShadow"
import {createReadBudget} from "./src/gateway/readBudget"
import {createSearchShadows, SEARCH_FIELDS} from "./src/gateway/searchShadow"
import {createSpaceShadow, createSpacesShadow, SPACE_FIELD, SPACES_FIELD} from "./src/gateway/spacesShadow"
import {createPipeGeoDbClient, createSpaceMap} from "./src/geodb/client"
import {readGeoDbCardConfig} from "./src/geodb/config"
import {createDerivedReader} from "./src/geodb/derived"
import {createGaiaIdMap, idMapQuery} from "./src/geodb/idMap"
import {readIdMapTwinConfig, withIdMapTwin} from "./src/geodb/idMapTwin"
import {createKeyedRecordsShadow, readKeyedRecordsShadowConfig} from "./src/geodb/keyedRecordsShadow"
import {
	createPostgresRankedReadsSource,
	createRankedReadsShadow,
	readRankedReadsShadowConfig,
} from "./src/geodb/rankedReadsShadow"
import {createRankingScoresShadow, readRankingScoresShadowConfig} from "./src/geodb/rankingScoresShadow"
import {createGeoDbRouter} from "./src/geodb/router"
import {createScoresShadow, readScoresShadowConfig} from "./src/geodb/scoresShadow"
import {createCliGeoDbSearchClient} from "./src/geodb/search"
import {health} from "./src/health"
import {createIpfsRouter} from "./src/ipfs"
import {setGraphqlSearchClient} from "./src/kg/graphqlSearchOpenSearchPlugin"
import {type GraphQLServerContext, getGraphqlPoolPressure, graphqlServer} from "./src/kg/postgraphile"
import {createRateLimiter} from "./src/middleware/rateLimit"
import {canonicalRequestLogging, isClientAbortError, requestId} from "./src/middleware/requestLogging"
import {
	createAccountWeightsSampler,
	createAccountWeightsShadow,
	readAccountWeightsShadowConfig,
} from "./src/pairFit/accountWeightsShadow"
import {createProfileRouter} from "./src/profile"
import {createProfileShadow, readProfileShadowConfig} from "./src/profile/shadow"
import {createProposalsRouter} from "./src/proposals"
import {createGovernanceQueriesShadow, readQueriesShadowConfig} from "./src/proposals/governance/queriesShadow"
import {createGovernanceSampler, readGovernanceSamplerConfig} from "./src/proposals/governance/sampler"
import {createGovernanceShadow, readGovernanceShadowConfig} from "./src/proposals/governance/shadow"
import {createSystemSpaceLookup} from "./src/proposals/governance/systemSpaces"
import {createSearchRouter} from "./src/search"
import {isPoolConnectTimeout} from "./src/services/dbFailures"
import {uploadEdit, uploadFile} from "./src/services/ipfs"
import {runtime} from "./src/services/runtime"
import {OpenSearchClient} from "./src/services/search"
import {db} from "./src/services/storage/storage"
import {log} from "./src/services/telemetry"
import {redactUrlCredentials} from "./src/utils/redactUrl"
import {createVersionedRouter} from "./src/versioned"
import {createVersionedV2Router} from "./src/versioned/v2"

type AppEnv = {
	Variables: {
		requestId: string
		graphqlOperationName?: string
		traceContext?: {
			traceId: string
			spanId: string
			traceFlags: number
		}
	}
}

const app = new Hono<AppEnv>()

// Hono dispatches all thrown errors through `app.onError` (see compose.js).
// Recognize client-side aborts (browser navigated away, AbortController.abort,
// React unmount cancelling an in-flight fetch) and short-circuit with a 499
// "Client Closed Request" — same convention nginx uses. Without this, Hono's
// default handler turns these into 500s, which downstream get logged as
// `log.error` and create Sentry issues for what is not a server fault.
//
// Real errors fall through to the default 500 path, where canonicalRequestLogging
// still emits the 5xx end log and Sentry issue.
app.onError((err, c) => {
	if (isClientAbortError(err)) {
		return new Response(null, {status: 499})
	}
	return c.text("Internal Server Error", 500)
})

function createGraphqlOverloadResponse(requestId: string) {
	const headers = new Headers({
		"content-type": "application/json",
		"retry-after": "1",
		"x-request-id": requestId,
	})

	return new Response(JSON.stringify({error: "database temporarily overloaded", requestId}), {
		status: 503,
		headers,
	})
}

// Request ID middleware (cheap, needed everywhere for correlation)
app.use("*", requestId())

// Let browsers reuse a preflight instead of repeating it before every GraphQL POST (GEO-3281).
// Without a max-age Chrome caches it for 5s, so each card query on /explore paid a fresh ~0.4s
// OPTIONS round trip. Browsers clamp this to their own ceiling (Chrome 2h, Firefox 24h).
app.use("*", cors({origin: "*", maxAge: 86400}))
log.info("HTTP compression disabled in API (managed by ingress)")

// Per-IP rate limiting (skips /health internally). No-op if VALKEY_URL is
// not set — see createRateLimiter.
const rateLimiter = createRateLimiter(process.env.VALKEY_URL, {
	windowMs: process.env.RATE_LIMIT_WINDOW_MS ? Number(process.env.RATE_LIMIT_WINDOW_MS) : undefined,
	maxRequests: process.env.RATE_LIMIT_MAX_REQUESTS ? Number(process.env.RATE_LIMIT_MAX_REQUESTS) : undefined,
})
if (rateLimiter) {
	app.use("*", rateLimiter)
}

// Health routes - no tracing (high frequency, low value)
app.route("/health", health)

// Apply canonical logging/tracing to API routes (not health)
// Health checks are high-frequency noise with low observability value
app.use("/ipfs/*", canonicalRequestLogging())
app.use("/profile/*", canonicalRequestLogging())
app.use("/search/*", canonicalRequestLogging())
app.use("/versioned/*", canonicalRequestLogging())
app.use("/v2/versioned/*", canonicalRequestLogging())
app.use("/proposals/*", canonicalRequestLogging())
app.use("/graphql", canonicalRequestLogging())

// Initialize search client with dependency injection
// Search is optional - if OPENSEARCH_URL is not set, search routes won't be added
const opensearchUrl = process.env.OPENSEARCH_URL
if (opensearchUrl) {
	// Validate URL format
	try {
		new URL(opensearchUrl)
	} catch (error) {
		log.error("Invalid OPENSEARCH_URL", {url: redactUrlCredentials(opensearchUrl)})
		throw error
	}

	// Get environment-specific index name; must mirror search-indexer-shared's
	// get_index_prefix(): staging -> staging_entities, testnet -> testnet_entities,
	// production -> entities
	const environment = process.env.ENVIRONMENT
	const baseIndexAlias = process.env.INDEX_ALIAS ?? "entities"
	const indexName =
		environment === "staging" || environment === "testnet" ? `${environment}_${baseIndexAlias}` : baseIndexAlias

	const searchClient = new OpenSearchClient(opensearchUrl, indexName)
	await searchClient.init()
	setGraphqlSearchClient(searchClient)
	app.route("/search", createSearchRouter(searchClient, runtime))
	log.info("Search routes enabled", {
		url: redactUrlCredentials(opensearchUrl),
		indexName,
		environment: environment ?? "production",
	})
} else {
	log.info("Search routes disabled - OPENSEARCH_URL not set")
}

// Mount versioned entities router
app.route("/versioned", createVersionedRouter(db, runtime))
log.info("Versioned entity routes enabled")

// Mount v2 versioned entities router (additive enrichments over v1)
app.route("/v2/versioned", createVersionedV2Router(db, runtime))
log.info("Versioned v2 entity routes enabled")

// Opt-in GraphQL gateway config; the gateway itself is built below, where /graphql is mounted.
const gatewayConfig = readGatewayConfig(process.env)
const gatewayShadowFields = new Set(
	[
		ENTITY_FIELD,
		ENTITIES_CONNECTION_FIELD,
		ENTITIES_ORDERED_FIELD,
		SPACES_FIELD,
		SPACE_FIELD,
		METAS_FIELD,
		...POINT_READ_FIELDS,
		...SEARCH_FIELDS,
	].filter((field) => gatewayConfig.mode === "on" && fieldModeOf(gatewayConfig, field) === "shadow"),
)
const gatewayShadowsAny = gatewayShadowFields.size > 0
// Fields served from geo-db (mode "on"). Only `entity` has a resolver; it answers one exact shape and falls back to
// PostGraphile for everything else (gaia_api_gateway_serve_fallback_total). geo-db's ids are turned back into gaia
// ids through the reverse id map in Postgres (geodb/idMap.ts, migration 0116; gaia_api_geodb_id_map_*).
const gatewayServeEntity = gatewayConfig.mode === "on" && fieldModeOf(gatewayConfig, ENTITY_FIELD) === "on"

// Mount the opt-in geo-db entity-card route (GEODB_ENTITY_CARDS=on|shadow; off by default)
const geoDbCardConfig = readGeoDbCardConfig(process.env)
// One geodb-client pipe pool and space map serve the card route and the gateway's entity shadow, so the
// pool's worker count, queue bound and timeout cap the two together.
const sharedGeoDb =
	geoDbCardConfig.mode !== "off" || gatewayShadowsAny || gatewayServeEntity
		? {geoDb: createPipeGeoDbClient(geoDbCardConfig), geoSpaceOf: createSpaceMap(geoDbCardConfig.spaceMapPath)}
		: null
if (geoDbCardConfig.mode !== "off") {
	app.route("/geodb", createGeoDbRouter(db, geoDbCardConfig, sharedGeoDb ?? {}))
	log.info("GeoDB entity card route enabled", {mode: geoDbCardConfig.mode, timeoutMs: geoDbCardConfig.timeoutMs})
}

// Mount profile router
// GEODB_PROFILE_SHADOW (off by default) also reads the profile's front page entity card from geo-db after serving a profile
// and counts whether its name and avatar url equal the served ones (gaia_api_profile_shadow_total). Never changes a response.
const profileShadowConfig = readProfileShadowConfig(process.env)
const profileShadow = profileShadowConfig.enabled
	? createProfileShadow({
			config: profileShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			geoSpaceOf: sharedGeoDb?.geoSpaceOf ?? createSpaceMap(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
app.route("/profile", createProfileRouter(db, runtime, profileShadow))
log.info("Profile routes enabled")

// Mount proposals router
// GEODB_GOVERNANCE_SHADOW (off by default) also reads the keyed governance records from geo-db after serving a proposal
// and counts whether the status they give equals the served one (gaia_api_governance_shadow_total). Never changes a response.
const governanceShadowConfig = readGovernanceShadowConfig(process.env)
const governanceShadow = governanceShadowConfig.enabled
	? createGovernanceShadow({
			config: governanceShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
// GEODB_GOVERNANCE_SHADOW_SAMPLER (off by default) also feeds that shadow proposals sampled from Postgres on a timer, so it
// compares without proposal traffic. Needs GEODB_GOVERNANCE_SHADOW on.
const governanceSamplerConfig = readGovernanceSamplerConfig(process.env)
if (governanceShadow && governanceSamplerConfig.enabled) {
	createGovernanceSampler({config: governanceSamplerConfig, db, shadow: governanceShadow, log}).start()
}
// GEODB_GOVERNANCE_QUERIES_SHADOW (off by default) twins the proposal list, detail (actions, votes) and active-check reads
// from the same records and counts them (gaia_api_governance_queries_shadow_total). Never changes a response.
const queriesShadowConfig = readQueriesShadowConfig(process.env)
const queriesShadow = queriesShadowConfig.enabled
	? createGovernanceQueriesShadow({
			config: queriesShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
app.route("/proposals", createProposalsRouter(db, runtime, governanceShadow, queriesShadow))
log.info("Proposals routes enabled")

// Private server-to-server routes (GEO-3140 For you, GEO-3224 pair fit). Mounted only when
// GAIA_INTERNAL_TOKEN is set, so without it /internal/* does not exist (404). Every request must
// carry the token; see src/forYou/router.ts. A short token is refused rather than mounted.
// GEODB_FOR_YOU_CONFIG_SHADOW (off by default) also reads the keyed for_you_config record from geo-db after the For you
// route reads its settings and counts whether it equals them (gaia_api_for_you_config_shadow_total). Never changes a response.
const forYouConfigShadowConfig = readForYouConfigShadowConfig(process.env)
const forYouConfigShadow = forYouConfigShadowConfig.enabled
	? createForYouConfigShadow({
			config: forYouConfigShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
// GEODB_ENTITY_RANKING_CONFIG_SHADOW (off by default) does the same for the tau_seconds the route served from
// entity_ranking_config (gaia_api_entity_ranking_config_shadow_total). Never changes a response.
const entityRankingConfigShadowConfig = readEntityRankingConfigShadowConfig(process.env)
const entityRankingConfigShadow = entityRankingConfigShadowConfig.enabled
	? createEntityRankingConfigShadow({
			config: entityRankingConfigShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
// GEODB_FEED_EXPERIMENT_SHADOW (off by default) compares the feed experiment the route served with geo-db's record of it
// (gaia_api_feed_experiment_shadow_total). Never changes a response.
const feedExperimentShadowConfig = readFeedExperimentShadowConfig(process.env)
const feedExperimentShadow = feedExperimentShadowConfig.enabled
	? createFeedExperimentShadow({
			config: feedExperimentShadowConfig,
			geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
			systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
			log,
		})
	: null
// GEODB_ACCOUNT_WEIGHTS_SHADOW (off by default) compares pair-fit candidates' account weight with geo-db's derived collection 5
// (gaia_api_account_weights_shadow_total), sampled and throttled, in the background. Never changes a response.
const accountWeightsShadowConfig = readAccountWeightsShadowConfig(process.env)
const accountWeightsShadow = accountWeightsShadowConfig.enabled
	? createAccountWeightsShadow({
			config: accountWeightsShadowConfig,
			reader: createDerivedReader(geoDbCardConfig),
			log,
		})
	: null
// The same flag also starts a sampler that reads a few account_weights rows from Postgres every GEODB_ACCOUNT_WEIGHTS_SHADOW_POLL_MS
// (60 s) and feeds them to the shadow, so it has samples even with no /pair-fit traffic.
if (accountWeightsShadow) {
	createAccountWeightsSampler({config: accountWeightsShadowConfig, db, shadow: accountWeightsShadow, log}).start()
}
// GEODB_SCORES_SHADOW (off by default) samples global_scores/local_scores/space_scores rows from Postgres on a timer and compares
// them with geo-db's derived collections 2/3/4 (gaia_api_scores_shadow_total). Background only; never touches a request.
const scoresShadowConfig = readScoresShadowConfig(process.env)
if (scoresShadowConfig.enabled) {
	createScoresShadow({config: scoresShadowConfig, db, reader: createDerivedReader(geoDbCardConfig), log}).start()
}
// GEODB_RANKING_SCORES_SHADOW (off by default) samples entity_ranking_scores against geo-db's engaged-only overlay in derived
// collection 21 (gaia_api_ranking_scores_shadow_total). Background only; never touches a request.
const rankingScoresShadowConfig = readRankingScoresShadowConfig(process.env)
if (rankingScoresShadowConfig.enabled) {
	createRankingScoresShadow({
		config: rankingScoresShadowConfig,
		db,
		reader: createDerivedReader(geoDbCardConfig),
		log,
	}).start()
}
// GEODB_RANKED_READS_SHADOW (off by default) runs the group B read twins (api/src/geodb/rankedReads) on Postgres rows and holds
// them to the SQL functions (gaia_api_ranked_read_shadow_total). Background only; reads stay on Postgres.
const rankedReadsShadowConfig = readRankedReadsShadowConfig(process.env)
if (rankedReadsShadowConfig.enabled) {
	createRankedReadsShadow({config: rankedReadsShadowConfig, source: createPostgresRankedReadsSource(db), log}).start()
}
// GEODB_KEYED_RECORDS_SHADOW (off by default) samples user_votes / ranks.rankings / ranks.ranking_blocks rows from Postgres on a
// timer and compares each with its keyed record in geo-db's votes Space (gaia_api_keyed_records_shadow_total). Background only.
const keyedRecordsShadowConfig = readKeyedRecordsShadowConfig(process.env)
if (keyedRecordsShadowConfig.enabled) {
	createKeyedRecordsShadow({
		config: keyedRecordsShadowConfig,
		db,
		geoDb: sharedGeoDb?.geoDb ?? createPipeGeoDbClient(geoDbCardConfig),
		systemSpaceOf: createSystemSpaceLookup(geoDbCardConfig.spaceMapPath),
		log,
	}).start()
}
const internalRoutes = mountInternalRoutes(
	app,
	db,
	process.env.GAIA_INTERNAL_TOKEN,
	forYouConfigShadow,
	entityRankingConfigShadow,
	feedExperimentShadow,
	accountWeightsShadow,
)
if (internalRoutes === "enabled") {
	log.info("Internal routes enabled")
} else if (internalRoutes === "too_short") {
	log.error("GAIA_INTERNAL_TOKEN is shorter than the minimum; internal routes disabled", {
		minimumLength: MIN_INTERNAL_TOKEN_LENGTH,
	})
} else {
	log.info("Internal routes disabled - GAIA_INTERNAL_TOKEN not set")
}

// Mount IPFS upload router
app.route("/ipfs", createIpfsRouter(uploadEdit, uploadFile, runtime))
log.info("IPFS routes enabled")

app.get("/", swaggerUI({url: "/openapi"}))

// Opt-in GraphQL gateway in front of PostGraphile (GRAPHQL_GATEWAY=on; off by default). A field in mode "on" is
// offered to its geo-db resolver first (see gateway/entityServe.ts); everything else is answered by PostGraphile; with entity=shadow (or entities, relation,
// relations, search, searchConnection) a sample of those operations is also compared against geo-db in the background.
// Wide reads (an entity read in every one of its spaces) share one budget of about one geo-db card read a second.
// The rates are per api pod and tunable (GATEWAY_SHADOW_READS_PER_SECOND, _BURST; every-Space: GATEWAY_SHADOW_EVERY_SPACE_PER_SECOND,
// _BURST): 11-06 the entity shadow's throttled refusals were 75 of its 85 unsupported_selection (88%).
const budgetNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isFinite(n) && n > 0 ? Math.min(n, max) : fallback
}
const wideReadBudget = createReadBudget({
	perSecond: budgetNumber(process.env.GATEWAY_SHADOW_READS_PER_SECOND, 1, 20),
	burst: budgetNumber(process.env.GATEWAY_SHADOW_READS_BURST, 5, 100),
})
// One gate for the holders lookup (GEODB_HOLDERS_LOOKUP): a store that refuses it is read the old way for a while.
const holdersGate = createHoldersGate()
// Every-Space reads (GEODB_EVERY_SPACE_READS) name the whole space map in one line: a budget of their own, smaller.
const everySpaceBudget = createReadBudget({
	perSecond: budgetNumber(process.env.GATEWAY_SHADOW_EVERY_SPACE_PER_SECOND, 0.5, 10),
	burst: budgetNumber(process.env.GATEWAY_SHADOW_EVERY_SPACE_BURST, 3, 50),
})
// GEODB_RANK_TYPED_SHADOW (off by default): `count` counts RANKING_SCORE_* ordered operations (gaia_api_ranking_score_orders_total),
// `on` also compares sampled typed pages with geo-db's derived collection 0 (gaia_api_rank_typed_shadow_*). Never answers a request.
const rankTypedConfig = readRankTypedConfig(process.env)
const rankTypedObserver =
	gatewayConfig.mode === "on" && rankTypedConfig.mode !== "off"
		? createRankTypedObserver({
				config: rankTypedConfig,
				reader: createDerivedReader(geoDbCardConfig),
				budget: createReadBudget({perSecond: rankTypedConfig.perSecond, burst: rankTypedConfig.burst}),
				log,
			})
		: null
const gatewayDeps: GatewayDeps<GraphQLServerContext> = {
	config: gatewayConfig,
	observers: rankTypedObserver ? [rankTypedObserver] : undefined,
	resolvers:
		gatewayServeEntity && sharedGeoDb
			? new Map([
					[
						ENTITY_FIELD,
						createEntityResolver({
							...sharedGeoDb,
							// GEODB_ID_MAP_TWIN (off by default) also asks geo-db's legacy-id read about each Postgres answer, in the background.
							gaiaIdsOf: withIdMapTwin(createGaiaIdMap(idMapQuery(db)), {
								client: sharedGeoDb.geoDb,
								config: readIdMapTwinConfig(process.env),
								log,
							}),
						}),
					],
				])
			: undefined,
	shadows:
		gatewayShadowsAny && sharedGeoDb
			? new Map([
					...(gatewayShadowFields.has(ENTITY_FIELD)
						? [
								[
									ENTITY_FIELD,
									createEntityShadow({
										...sharedGeoDb,
										log,
										budget: wideReadBudget,
										multiSpace: geoDbCardConfig.multiSpaceReads === true,
										backlinks: geoDbCardConfig.backlinkReads === true,
										...(geoDbCardConfig.everySpaceReads === true
											? {
													allGeoSpaces: sharedGeoDb.geoSpaceOf.all,
													everySpaceBudget,
													...(geoDbCardConfig.holdersLookup === true
														? {holders: holdersGate}
														: {}),
													everySpaceChunks: geoDbCardConfig.everySpaceChunks === true,
													...(geoDbCardConfig.everySpaceTimeoutMs
														? {everySpaceTimeoutMs: geoDbCardConfig.everySpaceTimeoutMs}
														: {}),
												}
											: {}),
									}),
								] as const,
							]
						: []),
					...createPointReadShadows(
						{
							...sharedGeoDb,
							log,
							budget: wideReadBudget,
							multiSpace: geoDbCardConfig.multiSpaceReads === true,
							entitiesBatch: geoDbCardConfig.entitiesBatch === true,
							derivedFields: geoDbCardConfig.derivedFields === true,
							orderPick: geoDbCardConfig.orderPick === true,
							typeScan: geoDbCardConfig.typeScan === true,
							relationsTypeFilter: geoDbCardConfig.relationsTypeFilter === true,
							...(geoDbCardConfig.everySpaceReads === true && geoDbCardConfig.everySpaceEntities === true
								? {
										allGeoSpaces: sharedGeoDb.geoSpaceOf.all,
										everySpaceBudget,
										...(geoDbCardConfig.holdersLookup === true ? {holders: holdersGate} : {}),
										everySpaceChunks: geoDbCardConfig.everySpaceChunks === true,
										...(geoDbCardConfig.everySpaceTimeoutMs
											? {everySpaceTimeoutMs: geoDbCardConfig.everySpaceTimeoutMs}
											: {}),
									}
								: {}),
						},
						POINT_READ_FIELDS.filter((field) => gatewayShadowFields.has(field)),
					),
					// entitiesConnection / entitiesOrderedByProperty =shadow only; absent from GRAPHQL_GATEWAY_FIELDS they are not mounted.
					...(
						[
							[ENTITIES_CONNECTION_FIELD, createEntitiesConnectionShadow],
							[ENTITIES_ORDERED_FIELD, createEntitiesOrderedShadow],
						] as const
					)
						.filter(([field]) => gatewayShadowFields.has(field))
						.map(
							([field, create]) =>
								[
									field,
									create({
										...sharedGeoDb,
										log,
										budget: wideReadBudget,
										multiSpace: geoDbCardConfig.multiSpaceReads === true,
										entitiesBatch: geoDbCardConfig.entitiesBatch === true,
										derivedFields: geoDbCardConfig.derivedFields === true,
										orderPick: geoDbCardConfig.orderPick === true,
										relationsTypeFilter: geoDbCardConfig.relationsTypeFilter === true,
										...(geoDbCardConfig.everySpaceReads === true &&
										geoDbCardConfig.everySpaceEntities === true
											? {
													allGeoSpaces: sharedGeoDb.geoSpaceOf.all,
													everySpaceBudget,
													...(geoDbCardConfig.holdersLookup === true
														? {holders: holdersGate}
														: {}),
													everySpaceChunks: geoDbCardConfig.everySpaceChunks === true,
													...(geoDbCardConfig.everySpaceTimeoutMs
														? {everySpaceTimeoutMs: geoDbCardConfig.everySpaceTimeoutMs}
														: {}),
												}
											: {}),
									}),
								] as const,
						),
					// spaces =shadow only: the Space list root, compared on held Spaces and their page / topic names.
					...(gatewayShadowFields.has(SPACES_FIELD)
						? [[SPACES_FIELD, createSpacesShadow({...sharedGeoDb, log})] as const]
						: []),
					// space =shadow: the single-Space root, the same comparison on one Space.
					...(gatewayShadowFields.has(SPACE_FIELD)
						? [[SPACE_FIELD, createSpaceShadow({...sharedGeoDb, log})] as const]
						: []),
					// metas =shadow: the indexer-cursor root, compared with derived collection 10 (`cursors`).
					...(gatewayShadowFields.has(METAS_FIELD)
						? [
								[
									METAS_FIELD,
									createMetasShadow({log, reader: createDerivedReader(geoDbCardConfig)}),
								] as const,
							]
						: []),
					...createSearchShadows(
						{
							geoSpaceOf: sharedGeoDb.geoSpaceOf,
							allGeoSpaces: sharedGeoDb.geoSpaceOf.all,
							log,
							search: createCliGeoDbSearchClient(geoDbCardConfig, {
								// Unset keeps the defaults (3000 ms, 2 at once). A novel multi-word query is cold on geo-db and can take 2-5 s.
								...(Number(process.env.GEODB_SEARCH_TIMEOUT_MS) > 0
									? {timeoutMs: Math.floor(Number(process.env.GEODB_SEARCH_TIMEOUT_MS))}
									: {}),
								...(Number(process.env.GEODB_SEARCH_MAX_CONCURRENT) > 0
									? {maxConcurrent: Math.floor(Number(process.env.GEODB_SEARCH_MAX_CONCURRENT))}
									: {}),
							}),
							mode: process.env.GEODB_SEARCH_MODE === "bm25" ? "bm25" : "typeahead",
							// GEODB_SEARCH_WORD_LEG=engine: one call, geo-db's own typeahead word leg widens it.
							wordLeg:
								process.env.GEODB_SEARCH_WORD_LEG?.trim().toLowerCase() === "engine"
									? "engine"
									: "client",
							// On unless GEODB_SEARCH_TYPE_FILTER=off: geo-db's hits lose the types Postgres excludes.
							excludeTypes: process.env.GEODB_SEARCH_TYPE_FILTER?.trim().toLowerCase() !== "off",
							readCards: sharedGeoDb.geoDb.readCards?.bind(sharedGeoDb.geoDb),
						},
						SEARCH_FIELDS.filter((field) => gatewayShadowFields.has(field)),
					),
				])
			: undefined,
	delegate: (request, context: GraphQLServerContext) => graphqlServer.fetch(request, context),
	onClassified: (classification, route) =>
		log.debug("GraphQL gateway routed operation", {...classification, route: route.kind, reason: route.reason}),
}
const graphqlHandler = createGateway(gatewayDeps)
// GATEWAY_PROBE (off by default) asks for `metas` and `relation(id:)` itself on a timer, through a second gateway built from
// the same shadows with the sample rate forced to 1, so their gaia_api_gateway_shadow_total series fill without client traffic.
// It never touches a request and never serves: no resolvers or observers, only the shadows.
const gatewayProbeConfig = readGatewayProbeConfig(process.env)
if (gatewayProbeConfig.enabled && gatewayConfig.mode === "on") {
	const probeGateway = createGateway({
		...gatewayDeps,
		config: {...gatewayConfig, shadowSampleRate: 1},
		resolvers: undefined,
		observers: undefined,
		random: () => 0,
	})
	createGatewayProbe({
		config: gatewayProbeConfig,
		db,
		send: (request) => probeGateway(request, {requestId: "gateway-probe"}),
		log,
	}).start()
}
if (gatewayConfig.mode === "on")
	log.info("GraphQL gateway enabled", {fields: Object.fromEntries(gatewayConfig.fieldModes)})

app.use("/graphql", async (c) => {
	const requestId = c.get("requestId") || "unknown"

	// Pool pressure shedding is handled inside usePgClient (only on cache misses).
	// This allows cached responses to be served even when the DB pool is saturated.
	try {
		return await graphqlHandler(c.req.raw, {
			traceContext: c.get("traceContext"),
			requestId,
			setGraphqlOperationName: (operationName: string) => {
				c.set("graphqlOperationName", operationName)
			},
		})
	} catch (error) {
		if (isPoolConnectTimeout(error) || (error instanceof Error && error.message === "pool_pressure_shed")) {
			const freshPoolPressure = getGraphqlPoolPressure()
			log.warn("GraphQL overloaded: pool pressure shed", {
				requestId,
				path: c.req.path,
				method: c.req.method,
				poolPressure: freshPoolPressure,
			})

			return createGraphqlOverloadResponse(requestId)
		}

		throw error
	}
})

app.get(
	"/openapi",
	openAPISpecs(app, {
		documentation: {
			info: {
				title: "Geo API",
				version: "1.0.0",
				description: "API for interacting with the Geo knowledge graph",
			},
			servers: [
				{url: "http://localhost:3000", description: "Local Server"},
				{
					url: "https://testnet-api.geobrowser.io",
					description: "Testnet Geo API",
				},
			],
			components: {
				schemas: {
					// Profile types
					Profile: {
						type: "object",
						description: "A user profile derived from their personal space",
						properties: {
							spaceId: {
								type: "string",
								format: "uuid",
								description: "The user's personal space ID",
							},
							name: {
								type: "string",
								nullable: true,
								description: "Display name from the NAME_PROPERTY value",
							},
							avatarUrl: {
								type: "string",
								nullable: true,
								description: "Avatar image URL from the AVATAR_PROPERTY relation",
							},
							address: {
								type: "string",
								description: "The user's wallet address (0x prefixed)",
							},
						},
						required: ["spaceId", "address"],
					},
					// Value types
					VersionedValue: {
						type: "object",
						description: "A value at a specific version. Only one value field will be set.",
						properties: {
							propertyId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							// Value columns (GRC-20 v2 data types) - only one will be set
							boolean: {type: "boolean", nullable: true},
							integer: {type: "integer", nullable: true},
							float: {type: "number", nullable: true},
							decimal: {type: "string", nullable: true},
							text: {type: "string", nullable: true},
							bytes: {
								type: "string",
								nullable: true,
								description: "Base64 encoded",
							},
							date: {type: "string", format: "date", nullable: true},
							time: {
								type: "string",
								nullable: true,
								description: "ISO 8601 time",
							},
							datetime: {type: "string", format: "date-time", nullable: true},
							schedule: {
								type: "object",
								nullable: true,
								description: "RFC 5545 schedule",
							},
							point: {
								type: "string",
								nullable: true,
								description: "WGS84 point",
							},
							rect: {
								type: "string",
								nullable: true,
								description: "WGS84 bounding box",
							},
							embedding: {type: "object", nullable: true},
							// Metadata
							language: {type: "string", nullable: true},
							unit: {type: "string", nullable: true},
							// Context metadata
							contextRootId: {type: "string", format: "uuid", nullable: true},
							contextEdgeTypeId: {
								type: "string",
								format: "uuid",
								nullable: true,
							},
						},
						required: ["propertyId", "spaceId"],
					},
					VersionedRelation: {
						type: "object",
						description: "A relation at a specific version (excluding block relations)",
						properties: {
							relationId: {type: "string", format: "uuid"},
							typeId: {type: "string", format: "uuid"},
							fromEntityId: {type: "string", format: "uuid"},
							fromSpaceId: {type: "string", format: "uuid", nullable: true},
							toEntityId: {type: "string", format: "uuid"},
							toSpaceId: {type: "string", format: "uuid", nullable: true},
							position: {type: "string", nullable: true},
							spaceId: {type: "string", format: "uuid"},
							verified: {type: "boolean", nullable: true},
							contextRootId: {type: "string", format: "uuid", nullable: true},
							contextEdgeTypeId: {
								type: "string",
								format: "uuid",
								nullable: true,
							},
						},
						required: ["relationId", "typeId", "fromEntityId", "toEntityId", "spaceId"],
					},
					BlockSnapshot: {
						type: "object",
						description: "A block snapshot - an entity linked via BLOCKS relation",
						properties: {
							id: {type: "string", format: "uuid"},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedValue"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedRelation"},
							},
						},
						required: ["id", "values", "relations"],
					},
					EntitySnapshot: {
						type: "object",
						description: "An entity snapshot at a specific version",
						properties: {
							id: {type: "string", format: "uuid"},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedValue"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/VersionedRelation"},
								description: "Excludes block relations",
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockSnapshot"},
							},
						},
						required: ["id", "values", "relations", "blocks"],
					},
					VersionEntry: {
						type: "object",
						description: "A version entry for listing versions",
						properties: {
							editId: {type: "string", format: "uuid"},
							blockNumber: {type: "string"},
							createdAt: {type: "string", format: "date-time"},
						},
						required: ["editId", "blockNumber", "createdAt"],
					},
					// Diff types
					DiffChunk: {
						type: "object",
						description: "A single chunk in a text diff",
						properties: {
							value: {type: "string"},
							added: {type: "boolean"},
							removed: {type: "boolean"},
						},
						required: ["value"],
					},
					ValueChange: {
						type: "object",
						description: "A value change with before/after values",
						properties: {
							propertyId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							type: {
								type: "string",
								enum: [
									"TEXT",
									"BOOL",
									"INT64",
									"FLOAT64",
									"DECIMAL",
									"BYTES",
									"DATE",
									"TIME",
									"DATETIME",
									"SCHEDULE",
									"POINT",
									"RECT",
									"EMBEDDING",
								],
							},
							before: {type: "string", nullable: true},
							after: {type: "string", nullable: true},
							diff: {
								type: "array",
								items: {$ref: "#/components/schemas/DiffChunk"},
								description: "Only present for TEXT type",
							},
						},
						required: ["propertyId", "spaceId", "type"],
					},
					RelationChange: {
						type: "object",
						description: "A relation change",
						properties: {
							relationId: {type: "string", format: "uuid"},
							typeId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							changeType: {type: "string", enum: ["ADD", "REMOVE", "UPDATE"]},
							before: {
								type: "object",
								nullable: true,
								properties: {
									toEntityId: {type: "string", format: "uuid"},
									toSpaceId: {type: "string", format: "uuid", nullable: true},
									position: {type: "string", nullable: true},
								},
								required: ["toEntityId"],
							},
							after: {
								type: "object",
								nullable: true,
								properties: {
									toEntityId: {type: "string", format: "uuid"},
									toSpaceId: {type: "string", format: "uuid", nullable: true},
									position: {type: "string", nullable: true},
								},
								required: ["toEntityId"],
							},
						},
						required: ["relationId", "typeId", "spaceId", "changeType"],
					},
					BlockChange: {
						type: "object",
						description: "A block change (text, image, or data block)",
						properties: {
							id: {type: "string", format: "uuid"},
							type: {
								type: "string",
								enum: ["textBlock", "imageBlock", "dataBlock"],
							},
							before: {type: "string", nullable: true},
							after: {type: "string", nullable: true},
							diff: {
								type: "array",
								items: {$ref: "#/components/schemas/DiffChunk"},
								description: "Only present for textBlock type",
							},
						},
						required: ["id", "type"],
					},
					GroupedEntityDiffResponse: {
						type: "object",
						description:
							"A grouped entity diff response. Dynamic group keys from the 'groups' object are spread at the root level.",
						properties: {
							entityId: {type: "string", format: "uuid"},
							name: {type: "string", nullable: true},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/ValueChange"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/RelationChange"},
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockChange"},
								description: "Static key for BLOCKS relation type changes",
							},
							groupKeys: {
								type: "array",
								items: {type: "string"},
								description: "Dynamic group keys present (excluding 'blocks')",
							},
						},
						additionalProperties: {
							type: "array",
							items: {$ref: "#/components/schemas/BlockChange"},
							description: "Dynamic groups by relation type ID",
						},
						required: ["entityId", "values", "relations", "blocks", "groupKeys"],
					},
					// Proposal status types
					ProposalStatusResponse: {
						type: "object",
						description: "Computed proposal status with vote counts and timing info",
						properties: {
							proposalId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							name: {
								type: "string",
								nullable: true,
								description: "Human-readable proposal name",
							},
							status: {
								type: "string",
								enum: ["PROPOSED", "EXECUTABLE", "ACCEPTED", "REJECTED"],
								description: "Current status of the proposal",
							},
							votingMode: {
								type: "string",
								enum: ["FAST", "SLOW"],
								description: "Voting mode determines threshold calculation",
							},
							votes: {
								type: "object",
								properties: {
									yes: {type: "integer", minimum: 0},
									no: {type: "integer", minimum: 0},
									abstain: {type: "integer", minimum: 0},
									total: {type: "integer", minimum: 0},
								},
								required: ["yes", "no", "abstain", "total"],
							},
							quorum: {
								type: "object",
								description: "Quorum progress information",
								properties: {
									required: {
										type: "integer",
										description: "Required votes for quorum",
									},
									current: {
										type: "integer",
										description: "Current total votes",
									},
									progress: {
										type: "number",
										description: "Progress as decimal (0.0 to 1.0)",
									},
									reached: {type: "boolean"},
								},
								required: ["required", "current", "progress", "reached"],
							},
							threshold: {
								type: "object",
								description: "Threshold progress information",
								properties: {
									required: {
										type: "string",
										description: "Required threshold (bigint as string)",
									},
									current: {
										type: "integer",
										description: "Current yes votes",
									},
									progress: {
										type: "number",
										description: "Progress as decimal (0.0 to 1.0)",
									},
									reached: {type: "boolean"},
								},
								required: ["required", "current", "progress", "reached"],
							},
							timing: {
								type: "object",
								properties: {
									startTime: {
										type: "integer",
										description: "Unix timestamp when voting starts",
									},
									endTime: {
										type: "integer",
										description: "Unix timestamp when voting ends",
									},
									timeRemaining: {
										type: "integer",
										nullable: true,
										description: "Seconds until voting ends, null if ended",
									},
									isVotingEnded: {type: "boolean"},
								},
								required: ["startTime", "endTime", "timeRemaining", "isVotingEnded"],
							},
							canExecute: {
								type: "boolean",
								description:
									"True if proposal can be executed on-chain: status is EXECUTABLE, which also requires the executeBy window to be open (now <= executeBy, or executeBy unset)",
							},
						},
						required: [
							"proposalId",
							"spaceId",
							"name",
							"status",
							"votingMode",
							"votes",
							"quorum",
							"threshold",
							"timing",
							"canExecute",
						],
					},
					ProposalListResponse: {
						type: "object",
						description: "Paginated list of proposal statuses",
						properties: {
							proposals: {
								type: "array",
								items: {$ref: "#/components/schemas/ProposalStatusResponse"},
							},
							nextCursor: {
								type: "string",
								nullable: true,
								description: "Cursor for next page, null if no more results",
							},
						},
						required: ["proposals", "nextCursor"],
					},
					ActiveProposalCheckResponse: {
						type: "object",
						description:
							"Whether an active (PROPOSED or EXECUTABLE) ADD_MEMBER or ADD_EDITOR proposal exists for the target in the given space",
						properties: {
							active: {
								type: "boolean",
								description:
									"True if at least one non-executed proposal with matching action type and target is currently in PROPOSED or EXECUTABLE status",
							},
						},
						required: ["active"],
					},
					// Proposal diff types
					EntityDiff: {
						type: "object",
						description: "A diff between two versions of an entity",
						properties: {
							entityId: {type: "string", format: "uuid"},
							name: {type: "string", nullable: true},
							values: {
								type: "array",
								items: {$ref: "#/components/schemas/ValueChange"},
							},
							relations: {
								type: "array",
								items: {$ref: "#/components/schemas/RelationChange"},
							},
							blocks: {
								type: "array",
								items: {$ref: "#/components/schemas/BlockChange"},
							},
						},
						required: ["entityId", "values", "relations", "blocks"],
					},
					PaginatedProposalDiff: {
						type: "object",
						description:
							"Paginated response for proposal diffs. Compares proposed changes against base state.",
						properties: {
							proposalId: {type: "string", format: "uuid"},
							spaceId: {type: "string", format: "uuid"},
							proposalStatus: {
								type: "string",
								enum: ["active", "closed", "executed"],
								description:
									"Proposal status: active (compare vs live), closed/executed (compare vs versioned at end_time)",
							},
							entities: {
								type: "array",
								items: {$ref: "#/components/schemas/EntityDiff"},
								description: "Entity diffs for this page",
							},
							pagination: {
								type: "object",
								properties: {
									cursor: {
										type: "string",
										nullable: true,
										description: "Base64-encoded cursor for next page",
									},
									hasMore: {type: "boolean"},
									totalEntities: {
										type: "integer",
										description: "Total number of affected entities",
									},
								},
								required: ["cursor", "hasMore", "totalEntities"],
							},
						},
						required: ["proposalId", "spaceId", "proposalStatus", "entities", "pagination"],
					},
				},
			},
		},
	}),
)

export default app
