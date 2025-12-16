import {Effect} from "effect"
import {encodeFunctionData} from "viem"
import {abi as DAOSpaceFactoryAbi} from "./dao-abi-v2"
import {getWalletClient} from "./client"
import {getChecksumAddresses} from "./std"

const DAO_SPACE_FACTORY_ADDRESS = "0xd5a61E983C40e0d33aaeE7b8b2DEB08179e0BFeF"

class DeployDaoSpaceError extends Error {
	readonly _tag = "DeployDaoSpaceError"
}

interface VotingSettings {
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
