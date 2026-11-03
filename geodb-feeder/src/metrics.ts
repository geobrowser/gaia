/** A tiny Prometheus text exposition: counters, gauges and one histogram, with labels. */
type Labels = Record<string, string>
const key = (l: Labels) =>
	Object.entries(l)
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([k, v]) => `${k}="${v.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`)
		.join(",")

class Series {
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

export class Histogram {
	private counts = BUCKETS.map(() => 0)
	private sum = 0
	private count = 0
	constructor(
		readonly name: string,
		readonly help: string,
	) {}
	observe(seconds: number): void {
		this.sum += seconds
		this.count += 1
		BUCKETS.forEach((b, i) => {
			if (seconds <= b) this.counts[i] = (this.counts[i] ?? 0) + 1
		})
	}
	render(): string {
		const lines = [`# HELP ${this.name} ${this.help}`, `# TYPE ${this.name} histogram`]
		BUCKETS.forEach((b, i) => lines.push(`${this.name}_bucket{le="${b}"} ${this.counts[i]}`))
		lines.push(`${this.name}_bucket{le="+Inf"} ${this.count}`, `${this.name}_sum ${this.sum}`, `${this.name}_count ${this.count}`)
		return lines.join("\n")
	}
}

export function createMetrics() {
	const counter = (name: string, help: string) => new Series(name, help, "counter")
	const gauge = (name: string, help: string) => new Series(name, help, "gauge")
	const m = {
		edits: counter("geodb_feeder_edits_total", "Kafka messages fully applied to geo-db, by outcome (applied, empty, dead_letter)."),
		intents: counter("geodb_feeder_intents_total", "Intents applied to geo-db."),
		chunks: counter("geodb_feeder_chunks_total", "geodb-client apply calls, by result (ok, retry, dead_letter)."),
		deadLetters: counter("geodb_feeder_dead_letters_total", "Messages or intents written to errors.jsonl and skipped."),
		serveRestarts: counter("geodb_feeder_serve_restarts_total", "geodb-serve restarts requested, by reason (found, exhausted)."),
		opsDetail: counter("geodb_feeder_translate_total", "Translator counters (skips, drops, predates-window)."),
		spaceMapPublish: counter("geodb_feeder_space_map_publish_total", "Space map ConfigMap publishes, by result."),
		offsetGaps: counter("geodb_feeder_offset_gaps_total", "Offsets skipped by the broker (retention or compaction)."),
		lag: gauge("geodb_feeder_consumer_lag", "Messages between the topic head and the last applied offset, by partition."),
		lastApplied: gauge("geodb_feeder_last_applied_timestamp_seconds", "Unix time the feeder last finished a message."),
		lastOffset: gauge("geodb_feeder_last_applied_offset", "Last applied offset, by partition."),
		spaces: gauge("geodb_feeder_spaces", "Spaces founded in geo-db."),
		serveUp: gauge("geodb_feeder_serve_up", "1 while geodb-serve answered the last probe."),
		consumerUp: gauge("geodb_feeder_consumer_running", "1 while the Kafka consumer is running."),
		apply: new Histogram("geodb_feeder_apply_seconds", "Wall time of one geodb-client apply."),
	}
	const render = () =>
		`${[...Object.values(m)].map((s) => s.render()).join("\n")}\n`
	return {...m, render}
}

export type Metrics = ReturnType<typeof createMetrics>
