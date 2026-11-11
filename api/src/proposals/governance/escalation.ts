/**
 * Read-time resolution of proposal settings escalations, the api's copy of the feeder's `resolveEscalations`
 * (geodb-feeder/src/proposalEscalationKeyed.ts). The api cannot import the feeder; `__tests__/governanceEscalation.test.ts`
 * runs both on the same inputs and fails if they diverge, so there is one behaviour, not two.
 */
export function resolveEscalations<V extends {block: bigint}, E extends {block: bigint; id: string}>(
	versions: V[],
	escalations: E[],
): {applied: Map<V, E>; orphans: E[]} {
	const applied = new Map<V, E>()
	const orphans: E[] = []
	const sorted = [...versions].sort((a, b) => (a.block < b.block ? -1 : a.block > b.block ? 1 : 0))
	const order = [...escalations].sort((a, b) =>
		a.block < b.block ? -1 : a.block > b.block ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
	)
	for (const e of order) {
		let target: V | undefined
		for (const v of sorted) if (v.block <= e.block) target = v
		if (target === undefined) orphans.push(e)
		else applied.set(target, e)
	}
	return {applied, orphans}
}
