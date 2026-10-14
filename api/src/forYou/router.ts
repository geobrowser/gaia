/**
 * `POST /internal/for-you` (GEO-3140, GEO-3144): re-rank Best's candidate window for one user.
 *
 * PRIVATE. Mounted only when GAIA_INTERNAL_TOKEN is set (otherwise the path does not exist and
 * answers 404), and every request must carry that token in `x-internal-token`, compared in
 * constant time. The only intended caller is the web app's server, which derives the user from a
 * verified Privy session; the browser never calls this. It is REST, not GraphQL, so it never passes
 * through the GraphQL response cache, and its answers are marked `private, no-store`.
 *
 * Request:  {userId, candidateIds, asOf?, pool?}
 *   userId        the user's personal space id
 *   candidateIds  Best's candidate window, in Best's order (at most MAX_CANDIDATES)
 *   asOf          optional ISO time to read interests at, so every page of one scroll agrees
 *   pool          optional (GEO-3299): {typeIds, debateTaggedClaims?, spaceIds?, createdAfter?,
 *                 depth?}. When present, the user's own topic candidates are walked with Explore's
 *                 filters and ranked together with Best's window, so For you can serve what Best's
 *                 window does not hold. Items then carry ids the caller did not send.
 *
 * Response: the ranking's name and version, the re-ordered items with their score parts, reason
 * and exploration marks, what was excluded and why, and the user's feed experiment (if any).
 * A user with no interest weights gets `personalized: false` and Best's order unchanged.
 */

import {createHash, timingSafeEqual} from "node:crypto"
import {Hono} from "hono"
import {canonicalRequestLogging} from "../middleware/requestLogging"
import {pairFitHandler} from "../pairFit/router"
import {log} from "../services/telemetry"
import {isValidUuid, normalizeUuid, toDashedUuid} from "../utils/uuid"
import {type Database, type ForYouInputs, type ForYouPool, readForYouInputs} from "./queries"
import {FOR_YOU_CODE_VERSION, FOR_YOU_POOL_CODE_VERSION, type ForYouRanking, rankForYou} from "./rank"

export const INTERNAL_TOKEN_HEADER = "x-internal-token"
/** Best's window is 66; room for a deeper one without letting a caller ask for thousands. */
export const MAX_CANDIDATES = 500
/** A shorter secret is refused at startup rather than mounted. */
export const MIN_INTERNAL_TOKEN_LENGTH = 32
/** Topic candidates taken by default, and the most a caller may ask for. */
export const DEFAULT_POOL_DEPTH = 200
export const MAX_POOL_DEPTH = 500
const MAX_POOL_TYPES = 20
/** Explore sends every space the reader can see. */
const MAX_POOL_SPACES = 2000

export function internalTokenMatches(expected: string, provided: string | null | undefined): boolean {
	if (!provided) return false
	// Hash both sides so the comparison is constant-time whatever the lengths.
	const a = createHash("sha256").update(expected).digest()
	const b = createHash("sha256").update(provided).digest()
	return timingSafeEqual(a, b)
}

export function forYouVersion(revision: number, pooled = false): string {
	return `for-you-${pooled ? FOR_YOU_POOL_CODE_VERSION : FOR_YOU_CODE_VERSION}.${revision}`
}

type Body = {userId: string; candidateIds: string[]; asOf: Date; pool: ForYouPool | null}

function parseUuidList(value: unknown, max: number, name: string): string[] | string {
	if (!Array.isArray(value)) return `${name} must be an array`
	if (value.length > max) return `at most ${max} ${name}`
	const ids = new Set<string>()
	for (const id of value) {
		if (typeof id !== "string" || !isValidUuid(id)) return `every ${name} entry must be a UUID`
		ids.add(normalizeUuid(id))
	}
	return [...ids].map(toDashedUuid)
}

export function parsePool(raw: unknown): ForYouPool | null | string {
	if (raw === undefined || raw === null) return null
	if (typeof raw !== "object" || Array.isArray(raw)) return "pool must be an object"
	const {typeIds, debateTaggedClaims, spaceIds, createdAfter, depth} = raw as Record<string, unknown>
	const types = parseUuidList(typeIds, MAX_POOL_TYPES, "pool.typeIds")
	if (typeof types === "string") return types
	if (types.length === 0) return "pool.typeIds must not be empty"
	let spaces: string[] | null = null
	if (spaceIds !== undefined && spaceIds !== null) {
		const parsed = parseUuidList(spaceIds, MAX_POOL_SPACES, "pool.spaceIds")
		if (typeof parsed === "string") return parsed
		// An empty list would match nothing; the caller means no narrowing by leaving it out.
		if (parsed.length === 0) return "pool.spaceIds must not be empty; omit it for no narrowing"
		spaces = parsed
	}
	let after: string | null = null
	if (createdAfter !== undefined && createdAfter !== null) {
		const text = typeof createdAfter === "number" ? String(createdAfter) : createdAfter
		if (typeof text !== "string" || !/^\d{1,12}$/.test(text)) return "pool.createdAfter must be epoch seconds"
		after = text
	}
	let poolDepth = DEFAULT_POOL_DEPTH
	if (depth !== undefined && depth !== null) {
		if (typeof depth !== "number" || !Number.isInteger(depth) || depth < 1 || depth > MAX_POOL_DEPTH)
			return `pool.depth must be an integer from 1 to ${MAX_POOL_DEPTH}`
		poolDepth = depth
	}
	if (debateTaggedClaims !== undefined && typeof debateTaggedClaims !== "boolean")
		return "pool.debateTaggedClaims must be a boolean"
	return {
		typeIds: types,
		debateTaggedClaims: debateTaggedClaims === true,
		spaceIds: spaces,
		createdAfter: after,
		depth: poolDepth,
	}
}

function parseBody(raw: unknown): Body | string {
	if (!raw || typeof raw !== "object") return "body must be a JSON object"
	const {userId, candidateIds, asOf, pool: rawPool} = raw as Record<string, unknown>
	if (typeof userId !== "string" || !isValidUuid(userId)) return "userId must be a UUID"
	if (!Array.isArray(candidateIds)) return "candidateIds must be an array"
	if (candidateIds.length > MAX_CANDIDATES) return `at most ${MAX_CANDIDATES} candidateIds`
	const ids: string[] = []
	const seen = new Set<string>()
	for (const id of candidateIds) {
		if (typeof id !== "string" || !isValidUuid(id)) return "every candidateId must be a UUID"
		const n = normalizeUuid(id)
		if (!seen.has(n)) {
			seen.add(n)
			ids.push(n)
		}
	}
	let when = new Date()
	if (asOf !== undefined && asOf !== null) {
		const parsed = typeof asOf === "string" ? new Date(asOf) : null
		if (!parsed || Number.isNaN(parsed.getTime())) return "asOf must be an ISO timestamp"
		// Pinning is for one scroll's pages, not for reading the past or the future.
		if (Math.abs(parsed.getTime() - when.getTime()) <= 24 * 3600 * 1000) when = parsed
	}
	const pool = parsePool(rawPool)
	if (typeof pool === "string") return pool
	return {userId: normalizeUuid(userId), candidateIds: ids, asOf: when, pool}
}

const dashless = (id: string) => (isValidUuid(id) ? normalizeUuid(id) : id)

export function buildResponse(body: Body, inputs: ForYouInputs) {
	const pooled = body.pool !== null
	const version = forYouVersion(inputs.settings.revision, pooled)
	const experiment = inputs.experiment
		? {
				id: inputs.experiment.id,
				arms: inputs.experiment.arms,
				interleaved: inputs.experiment.interleaved,
				assignment: inputs.experiment.assignment,
			}
		: null
	const personalized = inputs.weights.length > 0
	let ranking: ForYouRanking | null = null
	if (personalized) {
		ranking = rankForYou({
			candidates: inputs.candidates.map((c) => ({
				...c,
				entityId: dashless(c.entityId),
				topicIds: c.topicIds.map(dashless),
			})),
			weights: inputs.weights.map((w) => ({
				...w,
				topicId: dashless(w.topicId),
				topObjectId: w.topObjectId ? dashless(w.topObjectId) : null,
				relatedViaTopicId: w.relatedViaTopicId ? dashless(w.relatedViaTopicId) : null,
			})),
			topicNames: new Map([...inputs.topicNames].map(([id, name]) => [dashless(id), name])),
			config: inputs.settings.config,
			tauSeconds: inputs.settings.tauSeconds,
			seed: `${body.userId}:${body.candidateIds.join(",")}`,
		})
	}
	return {
		ranking: {
			name: "for-you",
			version,
			codeVersion: pooled ? FOR_YOU_POOL_CODE_VERSION : FOR_YOU_CODE_VERSION,
			configRevision: inputs.settings.revision,
		},
		personalized,
		asOf: body.asOf.toISOString(),
		config: inputs.settings.config,
		interestTopicCount: inputs.weights.length,
		items: ranking?.items ?? [],
		excluded: ranking?.excluded ?? [],
		exploration: ranking?.exploration ?? null,
		/** GEO-3299: how the pool was made; null on an unpooled request. */
		pool: inputs.pool,
		experiment,
	}
}

export function createInternalRouter(db: Database, token: string) {
	const router = new Hono()

	router.use("*", async (c, next) => {
		if (!internalTokenMatches(token, c.req.header(INTERNAL_TOKEN_HEADER))) {
			return c.json({error: "unauthorized"}, 401, {"cache-control": "no-store"})
		}
		await next()
	})

	router.post("/for-you", async (c) => {
		let raw: unknown
		try {
			raw = await c.req.json()
		} catch {
			return c.json({error: "invalid JSON"}, 400, {"cache-control": "no-store"})
		}
		const body = parseBody(raw)
		if (typeof body === "string") return c.json({error: body}, 400, {"cache-control": "no-store"})

		const started = performance.now()
		try {
			const inputs = await readForYouInputs(db, {
				userId: toDashedUuid(body.userId),
				candidateIds: body.candidateIds.map(toDashedUuid),
				asOf: body.asOf,
				pool: body.pool,
			})
			const response = buildResponse(body, inputs)
			log.info("for-you ranked", {
				candidates: body.candidateIds.length,
				personalized: response.personalized,
				excluded: response.excluded.length,
				explored: response.exploration?.picked ?? 0,
				poolTopics: response.pool?.topics ?? null,
				version: response.ranking.version,
				durationMs: Math.round(performance.now() - started),
			})
			return c.json(response, 200, {"cache-control": "private, no-store"})
		} catch (error) {
			log.error("for-you failed", {error: String(error)})
			return c.json({error: "for_you_unavailable"}, 500, {"cache-control": "no-store"})
		}
	})

	// GEO-3224: debate pair fit, on the same private router and behind the same token.
	router.post("/pair-fit", pairFitHandler(db))

	return router
}

/**
 * Mounts `/internal/*` on `app` only when `token` is set and long enough; otherwise those paths do
 * not exist and answer 404. Returns what it did, for the startup log.
 */
export function mountInternalRoutes(
	// biome-ignore lint/suspicious/noExplicitAny: mounts on any app, whatever its env
	app: Hono<any>,
	db: Database,
	token: string | undefined | null,
): "enabled" | "too_short" | "unset" {
	const trimmed = token?.trim()
	if (!trimmed) return "unset"
	if (trimmed.length < MIN_INTERNAL_TOKEN_LENGTH) return "too_short"
	app.use("/internal/*", canonicalRequestLogging())
	app.route("/internal", createInternalRouter(db, trimmed))
	return "enabled"
}
