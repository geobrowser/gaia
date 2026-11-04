/**
 * Consumer-group offset commits for the feeder's consumers. The offsets file is the feeder's own
 * truth; the group offset is what kafka-exporter (and so kafka_consumergroup_lag) reads. Every
 * finished message counts, including ones skipped because the offsets file already has them, and
 * each batch ends with a flush, so the group never sits behind a caught-up consumer. Throttled
 * per partition, so one busy partition cannot starve another's commits.
 */
export function createGroupCommitter(
	commit: (partition: number, nextOffset: string) => Promise<void>,
	intervalMs = 5000,
	now: () => number = Date.now,
) {
	const pending = new Map<number, number>()
	const committedAt = new Map<number, number>()
	const send = async (partition: number) => {
		const offset = pending.get(partition)
		if (offset === undefined) return
		pending.delete(partition)
		committedAt.set(partition, now())
		await commit(partition, String(offset + 1))
	}
	return {
		/** A message at `offset` is finished (applied or already done). Commits when the partition's interval has passed. */
		done: async (partition: number, offset: number) => {
			pending.set(partition, offset)
			if (now() - (committedAt.get(partition) ?? -Infinity) > intervalMs) await send(partition)
		},
		/** Commits whatever is still pending; call at the end of each batch. */
		flush: async () => {
			for (const p of [...pending.keys()]) await send(p)
		},
	}
}
