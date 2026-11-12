/**
 * Reads geo-db's fast-path execution fact (collection 31) for the Fast proposals a response is about to serve with no
 * `executed_at`, and fills `executedAt` from it through `resolveExecutedAt`. Mounted only when
 * `PROPOSAL_EXECUTED_AT_FROM_GEODB` is "true" (off by default, see `main.ts`), so with the switch off no router has this
 * and nothing here runs.
 *
 * Fail open: a stored `executed_at` always wins, and a read that fails or times out leaves the Postgres value, so the
 * response is what Postgres alone would have served. What it does not change: the list's status filter and ordering run
 * in SQL on `executed_at`, so a fact-only proposal is still filtered as unexecuted there.
 */
import type {DerivedReader} from "../geodb/derived"
import {decodeFastPathExecutionFact, type FastPathExecutionFact, resolveExecutedAt} from "./status"

/** Collection 31 of geo-db's derived class, `proposal_fast_executions`. */
export const FASTPATH_EXECUTIONS_COLLECTION = 31

/** Most facts one response reads; a list past this keeps Postgres' value for the rest. */
export const FASTPATH_READ_MAX = 50

export interface FastPathFacts {
	/** `proposals` with `executedAt` filled from the fact where Postgres has none. Never throws. */
	apply<T extends {id: string; votingMode: string; executedAt: bigint | null}>(proposals: T[]): Promise<T[]>
}

/** The collection key of a proposal: its entity id, sixteen bytes, as lowercase hex. */
export function proposalFactKey(proposalId: string): string | null {
	const hex = proposalId.replace(/-/g, "").toLowerCase()
	return /^[0-9a-f]{32}$/.test(hex) ? hex : null
}

export function createFastPathFacts(
	reader: DerivedReader,
	log: {warn: (message: string, fields?: Record<string, unknown>) => void} = {warn: () => {}},
): FastPathFacts {
	async function factOf(proposalId: string): Promise<FastPathExecutionFact | null> {
		const key = proposalFactKey(proposalId)
		if (key === null) return null
		try {
			const row = await reader.get(FASTPATH_EXECUTIONS_COLLECTION, key)
			return row === null ? null : decodeFastPathExecutionFact(row.value)
		} catch (error) {
			log.warn("fast-path execution fact read failed", {
				error: error instanceof Error ? error.message : String(error),
			})
			return null
		}
	}

	return {
		async apply(proposals) {
			const wanted = proposals
				.filter((p) => p.votingMode === "Fast" && p.executedAt === null)
				.slice(0, FASTPATH_READ_MAX)
				.map((p) => p.id)
			if (wanted.length === 0) return proposals
			const facts = new Map<string, FastPathExecutionFact | null>(
				await Promise.all(wanted.map(async (id) => [id, await factOf(id)] as const)),
			)
			return proposals.map((p) => {
				const fact = facts.get(p.id)
				if (fact === undefined) return p
				return {...p, executedAt: resolveExecutedAt(p.executedAt, fact, true)}
			})
		},
	}
}
