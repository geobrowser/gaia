/**
 * Shadow comparison of members and editors per Space, Postgres vs geo-db, behind GEODB_MEMBERSHIP_SHADOW_COMPARE.
 * The full id sets are compared, not counts: the card read is one entity's active statements, by property, and a
 * statement id is derived from (space, member, role), so each Postgres row maps to the one statement id geo-db
 * must hold. Equal counts with different members is a mismatch.
 * A mismatch is expected while the feeder lags the kg-indexer and before the membership backfill has run; read
 * it as a trend, and a persistent mismatch on a quiet Space as a bug.
 */
import {type EntityReader, readWithRetry} from "./geodbPipe.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {ROLE_PROPERTY, type Role, statementId, subjectEntity} from "./membership.js"
import type {SpaceRecord} from "./state.js"

export type CompareDeps = {
	/** Member ids (dashes stripped, lowercase) per Space and role. */
	pgMembers: () => Promise<Map<string, Record<Role, string[]>>>
	reader: EntityReader
	spaces: () => Record<string, SpaceRecord>
	/** Spaces the membership journal holds, so a Space Postgres now has no rows for is still compared (to 0). */
	journalSpaces: () => string[]
	metrics: Metrics
	sleep?: (ms: number) => Promise<void>
	/**
	 * Where the rows live and under which ids. Default: the legacy dual write (each Space's own geo-db Space,
	 * `statementId`). Under GEODB_RECORD_KEYS the keyed consumer writes every Space's rows to the `governance`
	 * system Space under `keyedStatementId` (membershipKeyed.ts), and main.ts passes that target instead.
	 */
	target?: CompareTarget
}

export type CompareTarget = {
	/** The geo-db Space holding `space`'s rows; nullish counts the Space as unmapped. */
	geoSpace: (space: string, spaces: Record<string, SpaceRecord>) => string | null | undefined
	statementId: (space: string, member: string, role: Role) => string
}

export const LEGACY_TARGET: CompareTarget = {
	geoSpace: (space, spaces) => spaces[space]?.geo,
	statementId,
}

/** `missing`: members Postgres has whose statement geo-db lacks. `extra`: statement ids geo-db has that no Postgres row maps to. Both capped at DIFF_CAP. */
export type Mismatch = {space: string; role: Role; pg: number; geo: number; missing: string[]; extra: string[]}

export const DIFF_CAP = 5
/** Mismatching Spaces named in the log line of one run. */
const LOG_CAP = 10

export type CompareSummary = {
	compared: number
	mismatched: Mismatch[]
	unmapped: number
	errors: number
}

const ROLES: Role[] = ["member", "editor"]

export async function compareMembership(deps: CompareDeps): Promise<CompareSummary> {
	const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
	const pg = await deps.pgMembers()
	const all = new Set([...pg.keys(), ...deps.journalSpaces()])
	const spaces = deps.spaces()
	const target = deps.target ?? LEGACY_TARGET
	const tally: Record<string, number> = {}
	const bump = (role: Role, result: string) => {
		deps.metrics.membershipShadowCompares.inc({role, result})
		tally[`${role}|${result}`] = (tally[`${role}|${result}`] ?? 0) + 1
	}
	const summary: CompareSummary = {compared: 0, mismatched: [], unmapped: 0, errors: 0}
	const diffIds: Record<string, number> = {}
	// A pipe holds grants only for the Spaces that existed when it opened.
	deps.reader.reset()
	for (const space of [...all].sort()) {
		const want = pg.get(space) ?? {member: [], editor: []}
		const geo = target.geoSpace(space, spaces)
		if (!geo) {
			summary.unmapped++
			for (const role of ROLES) bump(role, "unmapped")
			continue
		}
		let rows: Awaited<ReturnType<EntityReader["entityRows"]>>
		try {
			rows = await readWithRetry(deps.reader, geo, subjectEntity(space), sleep)
		} catch (e) {
			summary.errors++
			for (const role of ROLES) bump(role, "error")
			log("warn", "membership shadow read failed", {space, error: String(e).slice(0, 200)})
			continue
		}
		for (const role of ROLES) {
			const got = new Set(rows.filter((r) => r.prop === ROLE_PROPERTY[role]).map((r) => r.statement))
			const expected = new Map(want[role].map((m) => [target.statementId(space, m, role), m]))
			const missing = [...expected].filter(([sid]) => !got.has(sid)).map(([, m]) => m)
			const extra = [...got].filter((sid) => !expected.has(sid))
			summary.compared++
			if (missing.length === 0 && extra.length === 0) bump(role, "match")
			else {
				bump(role, "mismatch")
				diffIds[`${role}|missing_in_geo`] = (diffIds[`${role}|missing_in_geo`] ?? 0) + missing.length
				diffIds[`${role}|extra_in_geo`] = (diffIds[`${role}|extra_in_geo`] ?? 0) + extra.length
				summary.mismatched.push({
					space,
					role,
					pg: expected.size,
					geo: got.size,
					missing: missing.sort().slice(0, DIFF_CAP),
					extra: extra.sort().slice(0, DIFF_CAP),
				})
			}
		}
	}
	for (const role of ROLES) {
		for (const result of ["match", "mismatch", "unmapped", "error"]) {
			deps.metrics.membershipShadowSpaces.set({role, result}, tally[`${role}|${result}`] ?? 0)
		}
		for (const side of ["missing_in_geo", "extra_in_geo"]) {
			deps.metrics.membershipShadowIds.set({role, side}, diffIds[`${role}|${side}`] ?? 0)
		}
	}
	deps.metrics.membershipShadowLastRun.set({}, Date.now() / 1000)
	log(summary.mismatched.length ? "warn" : "info", "membership shadow comparison", {
		compared: summary.compared,
		mismatched: summary.mismatched.length,
		unmapped: summary.unmapped,
		errors: summary.errors,
		sample: summary.mismatched.slice(0, LOG_CAP),
	})
	return summary
}
