/**
 * The timer shared by the config shadows (for_you_config, entity_ranking_config, feed_experiments, the three entity-feed
 * config tables and the config revision). Each of them compares a keyed config record in geo-db with its Postgres
 * reference. A request cannot drive that: the tables change by migration or operator SQL, and a per-request hook gave three
 * samples a day. So a shadow supplies a `pass` (read the reference from Postgres, read the record from geo-db, count the
 * outcome) and this runs it on an interval, in the background, never overlapping itself and never throwing.
 *
 * The timer is `unref`ed, so it never keeps the process alive, and `start()` returns the function that clears it; a flag that
 * is off starts nothing. A pass still running when the next tick fires is skipped (`onBusy`), not queued.
 */

/** A few minutes: the sampled tables change by migration or operator SQL, so a pass a minute would only repeat itself. */
export const DEFAULT_SAMPLER_INTERVAL_MS = 300_000
const MAX_SAMPLER_INTERVAL_MS = 86_400_000

const isOn = (raw: string | undefined): boolean => ["1", "on", "true"].includes(raw?.trim().toLowerCase() ?? "")

/** `{enabled, intervalMs}` from a shadow's flag and its `_INTERVAL_MS` variable; a bad or missing interval takes the default. */
export function readIntervalSamplerConfig(
	env: Record<string, string | undefined>,
	flag: string,
	intervalVar: string,
	defaultIntervalMs: number = DEFAULT_SAMPLER_INTERVAL_MS,
): {enabled: boolean; intervalMs: number} {
	const n = Number(env[intervalVar])
	return {
		enabled: isOn(env[flag]),
		intervalMs: Number.isInteger(n) && n >= 1 ? Math.min(n, MAX_SAMPLER_INTERVAL_MS) : defaultIntervalMs,
	}
}

export type IntervalSampler = {
	/** One pass. Resolves when done; never throws. A call while a pass is running does nothing but `onBusy`. */
	runOnce(): Promise<void>
	/** Start the periodic pass; returns the stop function. Starts nothing when `enabled` is false. */
	start(): () => void
}

export function createIntervalSampler(deps: {
	enabled: boolean
	intervalMs: number
	pass: () => Promise<void>
	onBusy?: () => void
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	label: string
}): IntervalSampler {
	let running = false

	async function runOnce(): Promise<void> {
		if (running) {
			deps.onBusy?.()
			return
		}
		running = true
		try {
			await deps.pass()
		} catch (error) {
			// A pass counts its own failures; this is the last line of defence for the process.
			deps.log.warn(`${deps.label} pass failed`, {error: String(error)})
		} finally {
			running = false
		}
	}

	return {
		runOnce,
		start() {
			if (!deps.enabled) return () => {}
			const timer = setInterval(() => void runOnce(), deps.intervalMs)
			timer.unref?.()
			return () => clearInterval(timer)
		},
	}
}
