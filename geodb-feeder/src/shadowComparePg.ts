/**
 * The Postgres side of the membership shadow compare (GEODB_MEMBERSHIP_SHADOW_COMPARE) is the only thing the running
 * feeder reads Postgres for. It is optional: without `DATABASE_URL` the comparison stays off (fail closed, per feature)
 * and every other consumer runs as usual. The feeder Deployment can therefore drop `DATABASE_URL` and
 * `DATABASE_SSL_CA` once the compare is switched off or retired.
 */
import {log} from "./log.js"
import {ReadOnlyPg, pgConfig} from "./pgSource.js"

/** A read-only Postgres handle for the compare, or null (after an error log) when `DATABASE_URL` is unset. */
export function shadowComparePg(env: NodeJS.ProcessEnv = process.env): ReadOnlyPg | null {
	const url = env.DATABASE_URL
	if (!url) {
		log("error", "GEODB_MEMBERSHIP_SHADOW_COMPARE is set without DATABASE_URL; comparison disabled")
		return null
	}
	return new ReadOnlyPg(pgConfig(url, env.DATABASE_SSL_CA))
}
