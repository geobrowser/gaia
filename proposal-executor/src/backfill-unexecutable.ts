/**
 * One-shot backfill: flag proposals that can never execute because their space's
 * DAO has no record of them.
 *
 * The migration copied prod's proposal history into the v2 database without
 * recreating those proposals on the v2 chain. Their stored votes still resolve to
 * EXECUTABLE, so the API reported `canExecute: true` and the UI promised a
 * "Pending execution" that reverts `CanNotExecute()` forever. This stamps
 * `proposals.unexecutable_at` on the ones that are provably in that state, which
 * moves them to a terminal REJECTED and stops the executor retrying them.
 *
 * Only ever *sets* the column, and only after an on-chain confirmation per
 * proposal. Clearing it (`UPDATE proposals SET unexecutable_at = NULL`) fully
 * reverses this — nothing is deleted, because these proposals carry real PUBLISH
 * actions whose IPFS content is often still recoverable.
 *
 * Absence is detected with `latestProposalVersion == 0`. The two obvious probes
 * are unusable: `canExecuteProposal` and `isSupportThresholdReached` each return
 * `false` for an unknown proposal id exactly as they do for one that merely has
 * not passed yet, so neither separates "can never execute" from "not yet". Two
 * further traps this guards against:
 *   - an `eth_call` to an address with **no code** returns empty data, which
 *     decodes as version 0 — i.e. it would brand every proposal in the space
 *     dead. Both the registry and the resolved DAO are code-checked first.
 *   - a space whose registry entry is missing resolves to the zero address, which
 *     is a registry problem, not evidence about the proposal.
 * Anything undeterminable is skipped, never flagged.
 *
 * Dry run by default; pass `--commit` to write.
 *
 *   DATABASE_URL=... RPC_URL=... CHAIN_ID=55516 SPACE_REGISTRY_ADDRESS=0x... \
 *     bun run src/backfill-unexecutable.ts [--commit] [--space <uuid>] [--limit N]
 */

import Pg from "pg"
import {createPublicClient, getAddress, type Hex, http} from "viem"
import {DAOSpaceAbi, getChain, SpaceRegistryAbi, type SupportedChainId} from "./contracts.js"

const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

/** Candidates: unexecuted, not already flagged, and past their voting window. */
const CANDIDATE_SQL = `
SELECT pc.id, pc.space_id AS "spaceId", pc.name
FROM proposals_current pc
WHERE pc.executed_at IS NULL
  AND pc.unexecutable_at IS NULL
  AND pc.end_time > 0
  AND $1::bigint > pc.end_time
  AND ($2::uuid IS NULL OR pc.space_id = $2::uuid)
ORDER BY pc.space_id, pc.end_time
LIMIT $3::int
`

interface Candidate {
	id: string
	spaceId: string
	name: string | null
}

/** UUID (dashed) -> the `bytes16` form the contracts take. */
function toBytes16(uuid: string): Hex {
	return `0x${uuid.replaceAll("-", "")}` as Hex
}

function parseArgs(argv: string[]) {
	const commit = argv.includes("--commit")
	const spaceIdx = argv.indexOf("--space")
	const limitIdx = argv.indexOf("--limit")
	return {
		commit,
		space: spaceIdx >= 0 ? (argv[spaceIdx + 1] ?? null) : null,
		limit: limitIdx >= 0 ? Number(argv[limitIdx + 1]) : 5000,
	}
}

async function main() {
	const {commit, space, limit} = parseArgs(process.argv.slice(2))

	const databaseUrl = process.env.DATABASE_URL
	const rpcUrl = process.env.RPC_URL
	const registry = process.env.SPACE_REGISTRY_ADDRESS
	const chainId = Number(process.env.CHAIN_ID) as SupportedChainId
	if (!databaseUrl || !rpcUrl || !registry || !chainId) {
		throw new Error("DATABASE_URL, RPC_URL, SPACE_REGISTRY_ADDRESS and CHAIN_ID are all required")
	}

	const client = createPublicClient({chain: getChain(chainId), transport: http(rpcUrl)})
	const registryAddress = getAddress(registry)

	// A codeless registry would make every space resolve to nothing, which in turn
	// would look like universal absence. Refuse rather than flag the whole table.
	const registryCode = await client.getCode({address: registryAddress})
	if (!registryCode || registryCode === "0x") {
		throw new Error(
			`No contract code at SPACE_REGISTRY_ADDRESS ${registryAddress} on chain ${chainId} — refusing to run.`,
		)
	}

	const db = new Pg.Client({connectionString: databaseUrl})
	await db.connect()

	try {
		const {rows: candidates} = await db.query<Candidate>(CANDIDATE_SQL, [
			Math.floor(Date.now() / 1000),
			space,
			limit,
		])
		console.log(`${candidates.length} candidate proposal(s)${space ? ` in space ${space}` : ""}`)

		// One DAO address (and code check) per space, not per proposal.
		const daoCache = new Map<string, Hex | null>()
		const resolveDao = async (spaceId: string): Promise<Hex | null> => {
			const cached = daoCache.get(spaceId)
			if (cached !== undefined) return cached
			let resolved: Hex | null = null
			try {
				const address = (await client.readContract({
					address: registryAddress,
					abi: SpaceRegistryAbi,
					functionName: "spaceIdToAddress",
					args: [toBytes16(spaceId)],
				})) as Hex
				if (address && address.toLowerCase() !== ZERO_ADDRESS) {
					const code = await client.getCode({address})
					if (code && code !== "0x") resolved = address
				}
			} catch (error) {
				console.warn(`  ! could not resolve DAO for space ${spaceId}: ${(error as Error).message}`)
			}
			daoCache.set(spaceId, resolved)
			return resolved
		}

		const absent: Candidate[] = []
		let present = 0
		let skipped = 0

		for (const candidate of candidates) {
			const dao = await resolveDao(candidate.spaceId)
			if (!dao) {
				skipped++
				continue
			}
			try {
				const version = await client.readContract({
					address: dao,
					abi: DAOSpaceAbi,
					functionName: "latestProposalVersion",
					args: [toBytes16(candidate.id)],
				})
				if (Number(version) === 0) absent.push(candidate)
				else present++
			} catch (error) {
				// Undeterminable is not absence.
				console.warn(`  ! probe failed for ${candidate.id}: ${(error as Error).message}`)
				skipped++
			}
		}

		console.log(`\non chain: ${present}   absent: ${absent.length}   undeterminable (skipped): ${skipped}`)
		for (const p of absent.slice(0, 20)) {
			console.log(`  absent  ${p.id}  ${p.spaceId}  ${p.name ?? "(no name)"}`)
		}
		if (absent.length > 20) console.log(`  … and ${absent.length - 20} more`)

		if (!commit) {
			console.log("\nDRY RUN — nothing written. Re-run with --commit to stamp unexecutable_at.")
			return
		}
		if (absent.length === 0) {
			console.log("\nNothing to flag.")
			return
		}

		const now = Math.floor(Date.now() / 1000)
		await db.query("BEGIN")
		try {
			// Re-check `unexecutable_at IS NULL` in the UPDATE so a concurrent run
			// cannot overwrite an existing stamp with a later timestamp.
			const {rowCount} = await db.query(
				`UPDATE proposals SET unexecutable_at = $1::bigint
				 WHERE id = ANY($2::uuid[]) AND unexecutable_at IS NULL AND executed_at IS NULL`,
				[now, absent.map((p) => p.id)],
			)
			await db.query("COMMIT")
			console.log(`\nFlagged ${rowCount} proposal(s) unexecutable at ${now}.`)
			console.log("Reverse with: UPDATE proposals SET unexecutable_at = NULL WHERE unexecutable_at = " + now)
		} catch (error) {
			await db.query("ROLLBACK")
			throw error
		}
	} finally {
		await db.end()
	}
}

main().catch((error) => {
	console.error(error)
	process.exit(1)
})
