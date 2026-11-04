/** A tiny Prometheus text exposition: counters, gauges and one histogram, with labels. */
type Labels = Record<string, string>
const key = (l: Labels) =>
	Object.entries(l)
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([k, v]) => `${k}="${v.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`)
		.join(",")

export class Series {
	readonly values = new Map<string, number>()
	constructor(
		readonly name: string,
		readonly help: string,
		readonly type: "counter" | "gauge",
	) {}
	inc(labels: Labels = {}, n = 1): void {
		const k = key(labels)
		this.values.set(k, (this.values.get(k) ?? 0) + n)
	}
	set(labels: Labels, v: number): void {
		this.values.set(key(labels), v)
	}
	render(): string {
		const lines = [`# HELP ${this.name} ${this.help}`, `# TYPE ${this.name} ${this.type}`]
		if (this.values.size === 0) lines.push(`${this.name} 0`)
		for (const [k, v] of this.values) lines.push(`${this.name}${k ? `{${k}}` : ""} ${v}`)
		return lines.join("\n")
	}
}

const BUCKETS = [0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30]
/** Kafka message timestamp to applied: 10 ms to a minute, past the 30 s a stalled consumer shows up at. */
const FRESHNESS_BUCKETS = [0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60]

export class Histogram {
	private counts: number[]
	private sum = 0
	private count = 0
	constructor(
		readonly name: string,
		readonly help: string,
		private readonly buckets: readonly number[] = BUCKETS,
	) {
		this.counts = buckets.map(() => 0)
	}
	observe(seconds: number): void {
		this.sum += seconds
		this.count += 1
		this.buckets.forEach((b, i) => {
			if (seconds <= b) this.counts[i] = (this.counts[i] ?? 0) + 1
		})
	}
	render(): string {
		const lines = [`# HELP ${this.name} ${this.help}`, `# TYPE ${this.name} histogram`]
		this.buckets.forEach((b, i) => lines.push(`${this.name}_bucket{le="${b}"} ${this.counts[i]}`))
		lines.push(`${this.name}_bucket{le="+Inf"} ${this.count}`, `${this.name}_sum ${this.sum}`, `${this.name}_count ${this.count}`)
		return lines.join("\n")
	}
}

/** Observe now minus a Kafka message timestamp (epoch ms, as kafkajs hands it over). A missing or future stamp is skipped. */
export function observeFreshness(h: Histogram, kafkaTimestampMs: string | undefined, nowMs = Date.now()): void {
	const sent = Number(kafkaTimestampMs)
	if (!Number.isFinite(sent) || sent <= 0) return
	h.observe(Math.max(0, nowMs - sent) / 1000)
}

export function createMetrics() {
	const counter = (name: string, help: string) => new Series(name, help, "counter")
	const gauge = (name: string, help: string) => new Series(name, help, "gauge")
	const m = {
		edits: counter("geodb_feeder_edits_total", "Kafka messages fully applied to geo-db, by outcome (applied, empty, dead_letter)."),
		intents: counter("geodb_feeder_intents_total", "Intents applied to geo-db."),
		chunks: counter("geodb_feeder_chunks_total", "geodb-client apply calls, by result (ok, retry, dead_letter)."),
		deadLetters: counter("geodb_feeder_dead_letters_total", "Messages or intents written to errors.jsonl and skipped."),
		opsDetail: counter("geodb_feeder_translate_total", "Translator counters (skips, drops, predates-window)."),
		spaceMapPublish: counter("geodb_feeder_space_map_publish_total", "Space map ConfigMap publishes, by result."),
		offsetGaps: counter("geodb_feeder_offset_gaps_total", "Offsets skipped by the broker (retention or compaction)."),
		lag: gauge("geodb_feeder_consumer_lag", "Messages between the topic head and the last applied offset, by partition."),
		lastApplied: gauge("geodb_feeder_last_applied_timestamp_seconds", "Unix time the feeder last finished a message."),
		lastOffset: gauge("geodb_feeder_last_applied_offset", "Last applied offset, by partition."),
		spaces: gauge("geodb_feeder_spaces", "Spaces founded in geo-db."),
		serveUp: gauge("geodb_feeder_serve_up", "1 while geodb-serve answered the last probe."),
		consumerUp: gauge("geodb_feeder_consumer_running", "1 while the Kafka consumer is running."),
		membershipEvents: counter("geodb_feeder_membership_events_total", "space.membership messages handled, by outcome (applied, noop, ignored, dead_letter)."),
		membershipLag: gauge("geodb_feeder_membership_consumer_lag", "space.membership messages between the topic head and the last applied offset, by partition."),
		membershipUp: gauge("geodb_feeder_membership_consumer_running", "1 while the space.membership consumer is running."),
		membershipShadowCompares: counter("geodb_feeder_membership_shadow_compares_total", "Per-Space member/editor count comparisons, Postgres vs geo-db, by role and result (match, mismatch, unmapped, error)."),
		membershipShadowSpaces: gauge("geodb_feeder_membership_shadow_spaces", "Spaces in the last comparison run, by role and result."),
		membershipShadowIds: gauge("geodb_feeder_membership_shadow_diff_ids", "Ids that differ in the last comparison run, summed over Spaces, by role and side (missing_in_geo: Postgres has it, geo-db does not; extra_in_geo: the reverse)."),
		membershipShadowLastRun: gauge("geodb_feeder_membership_shadow_last_run_timestamp_seconds", "Unix time the last membership comparison run finished."),
		proposalEvents: counter("geodb_feeder_proposal_events_total", "space.governance messages handled, by outcome (applied, noop, ignored, unmapped, dead_letter)."),
		proposalsLag: gauge("geodb_feeder_proposals_consumer_lag", "space.governance messages between the topic head and the last applied offset, by partition."),
		proposalsUp: gauge("geodb_feeder_proposals_consumer_running", "1 while the space.governance consumer is running."),
		spaceEntityEvents: counter("geodb_feeder_space_entity_events_total", "space.creations messages handled, by outcome (applied, noop, ignored, pending, dead_letter). pending = Space not founded yet; retried once it is."),
		spaceEntitiesLag: gauge("geodb_feeder_space_entities_consumer_lag", "space.creations messages between the topic head and the last applied offset, by partition."),
		spaceEntitiesUp: gauge("geodb_feeder_space_entities_consumer_running", "1 while the space.creations consumer is running."),
		spaceEntitiesPending: gauge("geodb_feeder_space_entities_pending", "Registered Spaces whose system entity waits for the Space to be founded in geo-db."),
		apply: new Histogram("geodb_feeder_apply_seconds", "Wall time of one geodb-client apply."),
		freshness: new Histogram("geodb_feeder_edit_freshness_seconds", "Kafka message timestamp to applied in geo-db, edits consumer. Includes backlog catch-up.", FRESHNESS_BUCKETS),
		proposalFreshness: new Histogram("geodb_feeder_proposal_freshness_seconds", "Kafka message timestamp to applied in geo-db, space.governance consumer.", FRESHNESS_BUCKETS),
		spaceEntityFreshness: new Histogram("geodb_feeder_space_entity_freshness_seconds", "Kafka message timestamp to applied in geo-db, space.creations consumer.", FRESHNESS_BUCKETS),
		membershipFreshness: new Histogram("geodb_feeder_membership_freshness_seconds", "Kafka message timestamp to applied in geo-db, space.membership consumer.", FRESHNESS_BUCKETS),

	}
	const render = () =>
		`${[...Object.values(m)].map((s) => s.render()).join("\n")}\n`
	return {...m, render}
}

export type Metrics = ReturnType<typeof createMetrics>
