import {MainVotingAbi, PersonalSpaceAdminAbi} from "@graphprotocol/grc-20/abis"
import {Effect} from "effect"
import {encodeFunctionData, stringToHex} from "viem"
import {Environment} from "../services/environment"
import {graphql} from "./graphql"

const query = (spaceId: string) => {
	return `
    query {
      space(id: "${spaceId}") {
        id
        type
        daoAddress
        mainVotingPluginAddress
        memberAccessPluginAddress
        personalSpaceAdminPluginAddress
        spacePluginAddress
      }
    }`
}

type NetworkResult = {
	space: {
		id: string
		type: "PERSONAL" | "PUBLIC"
		daoAddress: string
		spacePluginAddress: string
		mainVotingPluginAddress: string | null
		memberAccessPluginAddress: string | null
		personalSpaceAdminPluginAddress: string | null
	} | null
}

export function getPublishEditCalldata(spaceId: string, cid: string, network: "TESTNET" | "MAINNET") {
	return Effect.gen(function* () {
		const config = yield* Environment
		// @TODO: Use correct endpoint
		const endpoint = ""
		// const endpoint = network === "TESTNET" ? config.API_ENDPOINT_TESTNET : config.API_ENDPOINT_MAINNET

		const result = yield* graphql<NetworkResult>({
			endpoint,
			query: query(spaceId),
		})

		if (!result.space) {
			return null
		}

		if (result.space.type === "PERSONAL") {
			const calldata = encodeFunctionData({
				functionName: "submitEdits",
				abi: PersonalSpaceAdminAbi,
				args: [cid, result.space.spacePluginAddress as `0x${string}`],
			})

			return {
				to: result.space.personalSpaceAdminPluginAddress,
				data: calldata,
			}
		}

		if (result.space.type === "PUBLIC") {
			const calldata = encodeFunctionData({
				functionName: "proposeEdits",
				abi: MainVotingAbi,
				args: [stringToHex(cid), cid, result.space.spacePluginAddress as `0x${string}`],
			})

			return {
				to: result.space.mainVotingPluginAddress as `0x${string}`,
				data: calldata,
			}
		}

		return null
	})
}
