/**
 * Shadow comparison of member and editor counts per Space, Postgres vs geo-db, behind GEODB_MEMBERSHIP_SHADOW_COMPARE.
 * Counts only (not membership sets): the card read is one entity's active statements, counted by property.
 * A mismatch is expected while the feeder lags the kg-indexer and before the membership backfill has run; read
 * it as a trend, and a persistent mismatch on a quiet Space as a bug.
 */
import {type EntityReader, readWithRetry} from "./geodbPipe.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {ROLE_PROPERTY, type Role, subjectEntity} from "./membership.js"
import type {SpaceRecord} from "./state.js"

export type CompareDeps = {
	pgCounts: () => Promise<Map<string, Record<Role, number>>>
	reader: EntityReader
	spaces: () => Record<string, SpaceRecord>
	/** Spaces the membership journal holds, so a Space Postgres now has no rows for is still compared (to 0). */
	journalSpaces: () => string[]
	metrics: Metrics
	sleep?: (ms: number) => Promise<void>
}

export type CompareSummary = {
	compared: number
	mismatched: {space: string; role: Role; pg: number; geo: number}[]
	unmapped: number
	errors: number
}

const ROLES: Role[] = ["member", "editor"]

export async function compareMembership(deps: CompareDeps): Promise<CompareSummary> {
	const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
	const pg = await deps.pgCounts()
	const all = new Set([...pg.keys(), ...deps.journalSpaces()])
	const spaces = deps.spaces()
	const tally: Record<string, number> = {}
	const bump = (role: Role, result: string) => {
		deps.metrics.membershipShadowCompares.inc({role, result})
		tally[`${role}|${result}`] = (tally[`${role}|${result}`] ?? 0) + 1
	}
	const summary: CompareSummary = {compared: 0, mismatched: [], unmapped: 0, errors: 0}
	// A pipe holds grants only for the Spaces that existed when it opened.
	deps.reader.reset()
	for (const space of [...all].sort()) {
		const want = pg.get(space) ?? {member: 0, editor: 0}
		const geo = spaces[space]?.geo
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
			const got = new Set(rows.filter((r) => r.prop === ROLE_PROPERTY[role]).map((r) => r.statement)).size
			summary.compared++
			if (got === want[role]) bump(role, "match")
			else {
				bump(role, "mismatch")
				summary.mismatched.push({space, role, pg: want[role], geo: got})
			}
		}
	}
	for (const role of ROLES) {
		for (const result of ["match", "mismatch", "unmapped", "error"]) {
			deps.metrics.membershipShadowSpaces.set({role, result}, tally[`${role}|${result}`] ?? 0)
		}
	}
	deps.metrics.membershipShadowLastRun.set({}, Date.now() / 1000)
	log(summary.mismatched.length ? "warn" : "info", "membership shadow comparison", {
		compared: summary.compared,
		mismatched: summary.mismatched.length,
		unmapped: summary.unmapped,
		errors: summary.errors,
		sample: summary.mismatched.slice(0, 10),
	})
	return summary
}
