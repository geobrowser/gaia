import {Effect} from "effect"
import {encodeFunctionData} from "viem"
import {abi as DAOSpaceFactoryAbi} from "./dao-abi-v2"
import {getWalletClient} from "./client"
import {getChecksumAddresses} from "./std"

const DAO_SPACE_FACTORY_ADDRESS = "0x86C773b693053D6899409f7deAb46ebd5FA0301c"

// Contract constants from DAOSpace.sol
export const RATIO_BASE = BigInt(10e6) // 10,000,000 (100% = 10e6, so 50% = 5e6)
export const MINIMUM_VOTING_DURATION = BigInt(2 * 24 * 60 * 60) // 2 days in seconds
export const MINIMUM_VOTING_DURATION_DAYS = 2

class DeployDaoSpaceError extends Error {
	readonly _tag = "DeployDaoSpaceError"
}

/**
 * User-friendly voting settings input (using percentages and days)
 */
export interface VotingSettingsInput {
	/** Percentage threshold for slow path (0-100) */
	slowPathPercentageThreshold: number
	/** Number of editors required for fast path approval */
	fastPathFlatThreshold: number
	/** Minimum number of editors required to vote */
	quorum: number
	/** Voting duration in days (minimum 2 days) */
	durationInDays: number
}

/**
 * Contract-level voting settings (using raw values)
 */
export interface VotingSettings {
	slowPathPercentageThreshold: bigint
	fastPathFlatThreshold: bigint
	quorum: bigint
	duration: bigint
}

interface DeployDaoSpaceArgs {
	votingSettings: VotingSettingsInput
	initialEditors: string[]
	initialMembers: string[]
}

/**
 * Convert a percentage (0-100) to the contract's ratio format
 */
export function percentageToRatio(percentage: number): bigint {
	return BigInt(Math.floor(percentage * 10e6 / 100))
}

/**
 * Convert days to seconds
 */
export function daysToSeconds(days: number): bigint {
	return BigInt(Math.floor(days * 24 * 60 * 60))
}

/**
 * Convert user-friendly voting settings to contract format
 */
export function toContractVotingSettings(input: VotingSettingsInput): VotingSettings {
	return {
		slowPathPercentageThreshold: percentageToRatio(input.slowPathPercentageThreshold),
		fastPathFlatThreshold: BigInt(input.fastPathFlatThreshold),
		quorum: BigInt(input.quorum),
		duration: daysToSeconds(input.durationInDays),
	}
}

export function validateVotingSettingsInput(settings: VotingSettingsInput, totalEditors: number): string | null {
	if (settings.slowPathPercentageThreshold < 0 || settings.slowPathPercentageThreshold > 100) {
		return "slowPathPercentageThreshold must be between 0 and 100"
	}
	if (settings.fastPathFlatThreshold < 0 || settings.fastPathFlatThreshold > totalEditors) {
		return `fastPathFlatThreshold must be between 0 and ${totalEditors} (number of initial editors)`
	}
	if (settings.quorum < 0 || settings.quorum > totalEditors) {
		return `quorum must be between 0 and ${totalEditors} (number of initial editors)`
	}
	if (settings.durationInDays < MINIMUM_VOTING_DURATION_DAYS) {
		return `durationInDays must be at least ${MINIMUM_VOTING_DURATION_DAYS} days`
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

		const validationError = validateVotingSettingsInput(args.votingSettings, initialEditors.length)
		if (validationError) {
			return yield* Effect.fail(new DeployDaoSpaceError(validationError))
		}

		const contractVotingSettings = toContractVotingSettings(args.votingSettings)
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
								slowPathPercentageThreshold: contractVotingSettings.slowPathPercentageThreshold,
								fastPathFlatThreshold: contractVotingSettings.fastPathFlatThreshold,
								quorum: contractVotingSettings.quorum,
								duration: contractVotingSettings.duration,
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
