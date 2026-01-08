import {MainVotingAbi, PersonalSpaceAdminAbi} from "@graphprotocol/grc-20/abis"
import {Effect} from "effect"
import {encodeFunctionData, stringToHex} from "viem"
import {Storage} from "../services/storage/storage"

// TODO(2026-02): Remove after legacy system sunset - this function uses old schema columns
export function getPublishEditCalldata(spaceId: string, cid: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			// Legacy: cast to old schema shape - columns exist in legacy DB but not new schema
			const maybeSpace = await client.query.spaces.findFirst({
				where: (spaces, {eq}) => eq(spaces.id, spaceId),
			}) as { id: string; type: string; spaceAddress: string; personalAddress: string | null; mainVotingAddress: string | null } | undefined

			if (!maybeSpace) {
				return null
			}

			if (maybeSpace.type === "Personal") {
				const calldata = encodeFunctionData({
					functionName: "submitEdits",
					abi: PersonalSpaceAdminAbi,
					args: [cid, '0x', maybeSpace.spaceAddress as `0x${string}`],
				})

				return {
					to: maybeSpace.personalAddress,
					data: calldata,
				}
			}

			if (maybeSpace.type === "Public") {
				const calldata = encodeFunctionData({
					functionName: "proposeEdits",
					abi: MainVotingAbi,
					args: [stringToHex(cid), cid, "0x", maybeSpace.spaceAddress as `0x${string}`],
				})

				return {
					to: maybeSpace.mainVotingAddress as `0x${string}`,
					data: calldata,
				}
			}
		})
	})
}
