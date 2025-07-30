import {
	type DAOFactory,
	DAOFactory__factory,
	DAORegistry__factory,
	PluginRepo__factory,
	PluginSetupProcessor__factory,
} from "@aragon/osx-ethers"
import {type ContextParams, type CreateDaoParams, DaoCreationSteps, PermissionIds} from "@aragon/sdk-client"
import {DaoCreationError, MissingExecPermissionError} from "@aragon/sdk-client-common"
import {id} from "@ethersproject/hash"
import {getChecksumAddress} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {encodeFunctionData, stringToHex, zeroAddress} from "viem"
import type {OmitStrict} from "../types"
import {abi as DaoFactoryAbi} from "./abi"
import {getPublicClient, getWalletClient} from "./client"
import {contracts, getDeployParams, type PluginInstallationWithViem, waitForSpaceToBeIndexed} from "./std"

class DeployDaoError extends Error {
	readonly _tag = "DeployDaoError"
}

export function deployAndValidateDao(params: CreateGeoDaoParams) {
	return Effect.gen(function* () {
		const dao = yield* Effect.tryPromise({
			try: async () => {
				const steps = createDao(params, getDeployParams())
				let dao = ""
				let pluginAddresses: string[] = []

				for await (const step of steps) {
					switch (step.key) {
						case DaoCreationSteps.CREATING:
							break
						case DaoCreationSteps.DONE: {
							dao = step.address
							pluginAddresses = step.pluginAddresses ?? []
						}
					}
				}

				return {dao, pluginAddresses}
			},
			catch: (e) => {
				console.error(`[SPACE][deploy] Failed creating DAO: ${e}`)
				return new DeployDaoError(`Failed creating DAO: ${e}`)
			},
		}).pipe(Effect.withSpan("deploySpace.createDao"), Effect.annotateSpans({...params}))

		yield* Effect.logInfo("[SPACE][deploy] Deployed DAO successfully!").pipe(
			Effect.annotateLogs({
				dao: dao.dao,
				pluginAddresses: dao.pluginAddresses,
			}),
		)

		yield* Effect.logInfo("[SPACE][deploy] Waiting for DAO to be indexed into a space").pipe(
			Effect.annotateLogs({dao: dao.dao, ...params}),
		)

		const waitResult = yield* waitForSpaceToBeIndexed(dao.dao).pipe(
			Effect.withSpan("deploySpace.waitForSpaceToBeIndexed"),
			Effect.annotateSpans({dao: dao.dao}),
		)

		yield* Effect.logInfo("[SPACE][deploy] Space indexed successfully").pipe(
			Effect.annotateLogs({
				dao: getChecksumAddress(dao.dao),
				pluginAddresses: dao.pluginAddresses,
				spaceId: waitResult,
			}),
		)

		return waitResult
	})
}

// encodeFunctionData it expects a hex bytes string.
export interface CreateGeoDaoParams extends OmitStrict<CreateDaoParams, "plugins"> {
	plugins: PluginInstallationWithViem[]
}

export async function* createDao(params: CreateGeoDaoParams, context: ContextParams) {
	if (!(context.signer && context.DAOFactory)) {
		return
	}

	const signer = context.signer

	const daoFactoryInstance = DAOFactory__factory.connect(context.DAOFactory, signer)

	const pluginInstallationData: DAOFactory.PluginSettingsStruct[] = []
	for (const plugin of params.plugins) {
		const repo = PluginRepo__factory.connect(plugin.id, signer)

		const currentRelease = await repo.latestRelease()
		const latestVersion = await repo["getLatestVersion(uint8)"](currentRelease)
		pluginInstallationData.push({
			pluginSetupRef: {
				pluginSetupRepo: repo.address,
				versionTag: latestVersion.tag,
			},
			data: plugin.data,
		})
	}

	// check if at least one plugin requests EXECUTE_PERMISSION on the DAO
	// This check isn't 100% correct all the time
	// simulate the DAO creation to get an address
	// const pluginSetupProcessorAddr = await daoFactoryInstance.pluginSetupProcessor();
	const pluginSetupProcessorAddress = contracts.PLUGIN_SETUP_PROCESSOR_ADDRESS
	const pluginSetupProcessor = PluginSetupProcessor__factory.connect(pluginSetupProcessorAddress, signer)
	let execPermissionFound = false

	// using the DAO base because it reflects a newly created DAO the best
	const daoBaseAddr = await daoFactoryInstance.daoBase()

	// simulates each plugin installation seperately to get the requested permissions
	for (const installData of pluginInstallationData) {
		const pluginSetupProcessorResponse = await pluginSetupProcessor.callStatic.prepareInstallation(
			daoBaseAddr,
			installData,
		)
		const found = pluginSetupProcessorResponse[1].permissions.find(
			(permission) => permission.permissionId === PermissionIds.EXECUTE_PERMISSION_ID,
		)
		if (found) {
			execPermissionFound = true
			break
		}
	}

	if (!execPermissionFound) {
		throw new MissingExecPermissionError()
	}

	const walletClient = getWalletClient()

	// We use viem as we run into unexpected "unknown account" errors when using ethers to
	// write the tx using the geo signer.
	const daoFactoryAddress = contracts.DAO_FACTORY_ADDRESS
	const hash = await walletClient.sendTransaction({
		to: daoFactoryAddress as `0x${string}`,
		data: encodeFunctionData({
			abi: DaoFactoryAbi,
			functionName: "createDao",
			args: [
				{
					subdomain: params.ensSubdomain ?? "",
					metadata: stringToHex(params.metadataUri),
					daoURI: params.daoUri ?? "",
					trustedForwarder: (params.trustedForwarder ?? zeroAddress) as `0x${string}`,
				},
				// @ts-expect-error mismatched types between ethers and viem. Ethers expects
				// the tag struct to be a BigNumberish but viem expects a string or number
				pluginInstallationData,
			],
		}),
	})

	// Commenting out the original implementation of DAO deployment. See the original here:
	// https://github.com/aragon/sdk/blob/36647d5d27ddc74b62892f829fec60e115a2f9be/modules/client/src/internal/client/methods.ts#L190
	// const tx = await daoFactoryInstance.connect(signer).createDao(
	//   {
	//     subdomain: params.ensSubdomain ?? '',
	//     metadata: stringToBytes(params.metadataUri),
	//     daoURI: params.daoUri ?? '',
	//     trustedForwarder: params.trustedForwarder ?? zeroAddress,
	//   },
	//   pluginInstallationData
	// );

	yield {
		key: DaoCreationSteps.CREATING,
		txHash: hash,
	}

	const publicClient = getPublicClient()
	const receipt = await publicClient.getTransactionReceipt({
		hash: hash,
	})

	const daoFactoryInterface = DAORegistry__factory.createInterface()
	const log = receipt.logs.find((l) => {
		const expectedId = daoFactoryInterface.getEventTopic("DAORegistered")
		return l.topics[0] === expectedId
	})

	if (!log) {
		console.error(`Failed to create DAO. Tx hash ${hash}`)
		throw new DaoCreationError()
	}

	// Plugin logs
	const pspInterface = PluginSetupProcessor__factory.createInterface()
	const installedLogs = receipt.logs?.filter(
		(e) => e.topics[0] === id(pspInterface.getEvent("InstallationApplied").format("sighash")),
	)

	// DAO logs
	const parsedLog = daoFactoryInterface.parseLog(log)
	if (!parsedLog.args["dao"]) {
		console.error(`Could not find DAO log. Tx hash ${hash}`)
		throw new DaoCreationError()
	}

	yield {
		key: DaoCreationSteps.DONE,
		address: parsedLog.args["dao"],
		pluginAddresses: installedLogs.map((log) => pspInterface.parseLog(log).args[1]),
	}
}
