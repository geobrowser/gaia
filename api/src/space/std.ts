// Using viem for the dao creation requires a slightly different encoding state for our plugins.
// When using ethers the type for `data` is expected to be a Uint8Array, but when using viem and
import {SupportedNetworks} from "@aragon/osx-commons-configs"

import {getChecksumAddress} from "@graphprotocol/grc-20"
import {MAINNET, TESTNET} from "@graphprotocol/grc-20/contracts"
import {Duration, Effect, Schedule} from "effect"
import {ethers, providers} from "ethers"
import {encodeAbiParameters, zeroAddress} from "viem"
import {EnvironmentLive} from "../services/environment"
import {Storage} from "../services/storage/storage"
import {getSigner} from "./client"

export const contracts = EnvironmentLive.chainId === "19411" ? TESTNET : MAINNET

const RATIO_BASE = ethers.BigNumber.from(10).pow(6) // 100% => 10**6
export const pctToRatio = (x: number) => RATIO_BASE.mul(x).div(100)

export function getDeployParams() {
	const daoFactory = contracts.DAO_FACTORY_ADDRESS
	const ensRegistry = contracts.ENS_REGISTRY_ADDRESS

	return {
		network: SupportedNetworks.LOCAL, // I don't think this matters but is required by Aragon SDK
		signer: getSigner(),
		web3Providers: new providers.JsonRpcProvider(EnvironmentLive.rpcEndpoint),
		DAOFactory: daoFactory,
		ENSRegistry: ensRegistry,
	}
}

class WaitForSpaceToBeIndexedError extends Error {
	readonly _tag = "WaitForSpaceToBeIndexedError"
}

export function waitForSpaceToBeIndexed(daoAddress: string) {
	const checkForSpace = Effect.gen(function* () {
		const db = yield* Storage
		const maybeSpace = yield* db.use(async (client) => {
			const result = await client.query.spaces.findFirst({
				where: (spaces, {eq}) => eq(spaces.daoAddress, getChecksumAddress(daoAddress)),
			})

			if (!result) {
				return null
			}

			return result.id
		})

		if (!maybeSpace) {
			return yield* Effect.fail(new WaitForSpaceToBeIndexedError("Could not find deployed space"))
		}

		return maybeSpace
	})

	return Effect.retry(
		checkForSpace,
		Schedule.exponential(100).pipe(
			Schedule.jittered,
			Schedule.compose(Schedule.elapsed),
			// Retry for 60 seconds.
			Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(60))),
		),
	)
}

// Using viem for the dao creation requires a slightly different encoding state for our plugins.
// When using ethers the type for `data` is expected to be a Uint8Array, but when using viem and
// encodeFunctionData it expects a hex bytes string.
export type PluginInstallationWithViem = {
	id: `0x${string}`
	data: `0x${string}`
}

export function getSpacePluginInstallItem({
	firstBlockContentUri,
	pluginUpgrader,
	precedessorSpace = zeroAddress,
}: {
	firstBlockContentUri: string
	pluginUpgrader: string
	precedessorSpace?: string
}): PluginInstallationWithViem {
	// from `encodeInstallationParams`
	const prepareInstallationInputs = [
		{
			internalType: "string",
			name: "_firstBlockContentUri",
			type: "string",
		},
		{
			internalType: "address",
			name: "_predecessorAddress",
			type: "address",
		},
		{
			internalType: "address",
			name: "_pluginUpgrader",
			type: "address",
		},
	]

	// This works but only if it's the only plugin being published. If we try multiple plugins with
	// the same upgrader we get an unpredictable gas limit
	const encodedParams = encodeAbiParameters(prepareInstallationInputs, [
		firstBlockContentUri,
		precedessorSpace,
		pluginUpgrader,
	])

	const spacePluginRepoAddress = contracts.SPACE_PLUGIN_REPO_ADDRESS

	return {
		id: spacePluginRepoAddress as `0x${string}`,
		data: encodedParams,
	}
}

export function getChecksumAddresses(addresses: string[]): `0x${string}`[] {
	return addresses.flatMap((e) => {
		try {
			const mapped = getChecksumAddress(e)
			return [mapped]
		} catch {
			return []
		}
	})
}

export function generateEditFormData(content: Uint8Array): FormData {
	// Convert to regular Uint8Array if needed for compatibility
	const buffer = new Uint8Array(content)
	const blob = new Blob([buffer], {
		type: "application/octet-stream",
	})
	const formData = new FormData()
	formData.append("file", blob)

	return formData
}
