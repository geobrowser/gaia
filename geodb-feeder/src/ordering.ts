/**
 * The backfill's precondition. It reads Postgres and writes only what geo-db lacks, and the feeder then
 * replays the stream from where it stopped. That is only safe when, for every partition:
 *
 * - the feeder is stopped (its consumer group has no members), so nothing else writes geo-db or the journals;
 * - the feeder stopped between messages (no `inflight` entry): a half-applied message resumes by intent index,
 *   and that index would be wrong once the backfill has changed what the journals know;
 * - the kg-indexer has committed past the feeder's last applied offset, so every Postgres row the backfill
 *   reads reflects at least every edit geo-db already holds. Each edit after that is replayed by the feeder
 *   on top of the backfilled rows, newest last.
 */
import type {Store} from "./state.js"

export type OffsetsAdmin = {
	fetchTopicOffsets(topic: string): Promise<{partition: number; low: string; high: string}[]>
	fetchOffsets(args: {groupId: string; topics: string[]}): Promise<{topic: string; partitions: {partition: number; offset: string}[]}[]>
	describeGroups(groupIds: string[]): Promise<{groups: {groupId: string; state: string; members: unknown[]}[]}>
}

export type OrderingCheck = {
	ok: boolean
	problems: string[]
	partitions: {partition: number; feederDone: number | null; indexerCommitted: number; high: number}[]
	feederGroupState: string
}

export async function checkOrdering(args: {
	admin: OffsetsAdmin
	topic: string
	feederGroup: string
	indexerGroup: string
	store: Pick<Store, "lastDone" | "inflight">
}): Promise<OrderingCheck> {
	const {admin, topic, store} = args
	const problems: string[] = []

	const group = (await admin.describeGroups([args.feederGroup])).groups.find((g) => g.groupId === args.feederGroup)
	const feederGroupState = group?.state ?? "Unknown"
	if (!group || group.members.length > 0 || !["Empty", "Dead"].includes(group.state)) {
		problems.push(`feeder group ${args.feederGroup} is ${feederGroupState} with ${group?.members.length ?? "?"} members: scale geodb-feeder to 0 first`)
	}

	const inflight = Object.entries(store.inflight)
	if (inflight.length > 0) {
		problems.push(
			`the feeder stopped inside a message (${inflight.map(([p, i]) => `p${p}@${i.offset}`).join(", ")}): scale it back to 1, let it finish, then stop it again`,
		)
	}

	const committed = new Map<number, number>()
	for (const t of await admin.fetchOffsets({groupId: args.indexerGroup, topics: [topic]})) {
		if (t.topic !== topic) continue
		for (const p of t.partitions) committed.set(p.partition, Number(p.offset))
	}
	const partitions: OrderingCheck["partitions"] = []
	for (const p of await admin.fetchTopicOffsets(topic)) {
		const feederDone = store.lastDone(p.partition)
		const indexerCommitted = committed.get(p.partition) ?? -1
		partitions.push({partition: p.partition, feederDone, indexerCommitted, high: Number(p.high)})
		// The committed offset is the next one the indexer will read: everything below it is in Postgres.
		const need = (feederDone ?? -1) + 1
		if (indexerCommitted < need) {
			problems.push(`partition ${p.partition}: kg-indexer committed ${indexerCommitted}, feeder applied through ${feederDone}; wait for the indexer`)
		}
	}
	return {ok: problems.length === 0, problems, partitions, feederGroupState}
}
