import {MainVotingAbi, PersonalSpaceAdminAbi} from "@graphprotocol/grc-20/abis"
import {Effect} from "effect"
import {encodeFunctionData, stringToHex} from "viem"
import {Storage} from "../services/storage/storage"

export function getPublishEditCalldata(spaceId: string, cid: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const maybeSpace = yield* db.use((client) =>
			client.query.spaces.findFirst({
				where: (spaces, {eq}) => eq(spaces.id, spaceId),
			}),
		)

		if (!maybeSpace) {
			return null
		}

		if (maybeSpace.type === "Personal") {
			const calldata = encodeFunctionData({
				functionName: "submitEdits",
				abi: PersonalSpaceAdminAbi,
				args: [cid, maybeSpace.spaceAddress as `0x${string}`],
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
				args: [stringToHex(cid), cid, maybeSpace.spaceAddress as `0x${string}`],
			})

			return {
				to: maybeSpace.mainVotingAddress as `0x${string}`,
				data: calldata,
			}
		}

		return null
	})
}
