import {MainVotingAbi, PersonalSpaceAdminAbi} from "@graphprotocol/grc-20/abis"
import {sql} from "drizzle-orm"
import {Effect} from "effect"
import {encodeFunctionData, stringToHex} from "viem"
import {Storage} from "../services/storage/storage"

// Legacy space type from old schema
interface LegacySpace {
	id: string
	type: "Personal" | "Public"
	space_address: string
	personal_address: string | null
	main_voting_address: string | null
}

// TODO(2026-02): Remove after legacy system sunset - this function uses old schema columns
export function getPublishEditCalldata(spaceId: string, cid: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			// Use raw SQL to query old schema columns that aren't in the new TypeScript schema
			const result = await client.execute(sql`
				SELECT id, type, space_address, personal_address, main_voting_address
				FROM spaces
				WHERE id = ${spaceId}
				LIMIT 1
			`)

			const maybeSpace = result.rows[0] as LegacySpace | undefined

			if (!maybeSpace) {
				return null
			}

			if (maybeSpace.type === "Personal") {
				const calldata = encodeFunctionData({
					functionName: "submitEdits",
					abi: PersonalSpaceAdminAbi,
					args: [cid, '0x', maybeSpace.space_address as `0x${string}`],
				})

				return {
					to: maybeSpace.personal_address,
					data: calldata,
				}
			}

			if (maybeSpace.type === "Public") {
				const calldata = encodeFunctionData({
					functionName: "proposeEdits",
					abi: MainVotingAbi,
					args: [stringToHex(cid), cid, "0x", maybeSpace.space_address as `0x${string}`],
				})

				return {
					to: maybeSpace.main_voting_address as `0x${string}`,
					data: calldata,
				}
			}
		})
	})
}
