import {Effect} from "effect"
import {encodeFunctionData} from "viem"
import {abi as DAOSpaceFactoryAbi} from "./dao-abi-v2"
import {getWalletClient} from "./client"
import {getChecksumAddresses} from "./std"

const DAO_SPACE_FACTORY_ADDRESS = "0x86C773b693053D6899409f7deAb46ebd5FA0301c"

// Contract constants from DAOSpace.sol
export const RATIO_BASE = BigInt(10000) // 100% = 10000 basis points
export const MINIMUM_VOTING_DURATION = BigInt(2 * 24 * 60 * 60) // 2 days in seconds

class DeployDaoSpaceError extends Error {
	readonly _tag = "DeployDaoSpaceError"
}

export interface VotingSettings {
	slowPathPercentageThreshold: bigint
	fastPathFlatThreshold: bigint
	quorum: bigint
	duration: bigint
}

interface DeployDaoSpaceArgs {
	votingSettings: VotingSettings
	initialEditors: string[]
	initialMembers: string[]
}

export function validateVotingSettings(settings: VotingSettings, totalEditors: number): string | null {
	if (settings.slowPathPercentageThreshold > RATIO_BASE) {
		return `slowPathPercentageThreshold must be <= ${RATIO_BASE} (${RATIO_BASE} = 100%)`
	}
	if (settings.fastPathFlatThreshold > BigInt(totalEditors)) {
		return `fastPathFlatThreshold must be <= number of initial editors (${totalEditors})`
	}
	if (settings.quorum > BigInt(totalEditors)) {
		return `quorum must be <= number of initial editors (${totalEditors})`
	}
	if (settings.duration < MINIMUM_VOTING_DURATION) {
		return `duration must be >= ${MINIMUM_VOTING_DURATION} seconds (2 days)`
	}
	return null
}

export function deployDaoSpace(args: DeployDaoSpaceArgs) {
	return Effect.gen(function* () {
		yield* Effect.logInfo("[DAO_SPACE][deploy] Deploying DAO space")

		const initialEditors = getChecksumAddresses(args.initialEditors)
		const initialMembers = getChecksumAddresses(args.initialMembers)

		if (initialEditors.length === 0) {
			return yield* Effect.fail(
				new DeployDaoSpaceError("At least one initial editor is required"),
			)
		}

		const validationError = validateVotingSettings(args.votingSettings, initialEditors.length)
		if (validationError) {
			return yield* Effect.fail(new DeployDaoSpaceError(validationError))
		}

		const walletClient = getWalletClient()

		yield* Effect.logInfo("[DAO_SPACE][deploy] Sending createDAOSpaceProxy transaction")

		const hash = yield* Effect.tryPromise({
			try: async () => {
				return await walletClient.sendTransaction({
					to: DAO_SPACE_FACTORY_ADDRESS as `0x${string}`,
					data: encodeFunctionData({
						abi: DAOSpaceFactoryAbi,
						functionName: "createDAOSpaceProxy",
						args: [
							{
								slowPathPercentageThreshold: args.votingSettings.slowPathPercentageThreshold,
								fastPathFlatThreshold: args.votingSettings.fastPathFlatThreshold,
								quorum: args.votingSettings.quorum,
								duration: args.votingSettings.duration,
							},
							initialEditors,
							initialMembers,
						],
					}),
				})
			},
			catch: (e) => {
				console.error(`[DAO_SPACE][deploy] Failed to send transaction: ${e}`)
				return new DeployDaoSpaceError(`Failed to send transaction: ${e}`)
			},
		}).pipe(Effect.withSpan("deployDaoSpace.sendTransaction"))

		yield* Effect.logInfo("[DAO_SPACE][deploy] Transaction sent").pipe(
			Effect.annotateLogs({hash}),
		)

		return hash
	})
}
