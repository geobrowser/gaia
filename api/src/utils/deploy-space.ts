import {SupportedNetworks} from "@aragon/osx-commons-configs"
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
import {Graph, Id, SystemIds, getChecksumAddress} from "@graphprotocol/grc-20"
import {MAINNET, TESTNET} from "@graphprotocol/grc-20/contracts"
import {EditProposal} from "@graphprotocol/grc-20/proto"
import {Duration, Effect, Either, Schedule} from "effect"
import {providers} from "ethers"
import {encodeAbiParameters, encodeFunctionData, stringToHex, zeroAddress} from "viem"
import type {OmitStrict} from "~/src/types"
import {Environment, EnvironmentLive} from "../services/environment"
import {upload} from "../services/ipfs"
import {abi as DaoFactoryAbi} from "./abi"
import {getPublicClient, getSigner, getWalletClient} from "./client"
import {graphql} from "./graphql"

const contracts = {
	TESTNET: TESTNET,
	MAINNET: MAINNET,
}

const getDeployParams = (network: "TESTNET" | "MAINNET") => {
	const daoFactory = contracts[network].DAO_FACTORY_ADDRESS
	const ensRegistry = contracts[network].ENS_REGISTRY_ADDRESS

	const rpcEndpoint = network === "TESTNET" ? EnvironmentLive.rpcEndpointTestnet : EnvironmentLive.rpcEndpointMainnet

	return {
		network: SupportedNetworks.LOCAL, // I don't think this matters but is required by Aragon SDK
		signer: getSigner(network),
		web3Providers: new providers.JsonRpcProvider(rpcEndpoint),
		DAOFactory: daoFactory,
		ENSRegistry: ensRegistry,
	}
}

class DeployDaoError extends Error {
	readonly _tag = "DeployDaoError"
}

class WaitForSpaceToBeIndexedError extends Error {
	readonly _tag = "WaitForSpaceToBeIndexedError"
}

interface DeployArgs {
	spaceName: string
	initialEditorAddress: string
	network: "TESTNET" | "MAINNET"
}

export function deploySpace(args: DeployArgs) {
	const {network} = args

	return Effect.gen(function* () {
		const config = yield* Environment
		yield* Effect.logInfo("[SPACE][deploy] Deploying space to " + network)
		const initialEditorAddress = getChecksumAddress(args.initialEditorAddress)

		const entityOp = Graph.createEntity({
			name: args.spaceName,
			types: [SystemIds.SPACE_TYPE],
		})

		const initialContent = EditProposal.encode({
			name: args.spaceName,
			author: initialEditorAddress,
			ops: entityOp.ops,
		})

		yield* Effect.logInfo("[SPACE][deploy] Uploading EDIT to IPFS")
		const blob = new Blob([initialContent], {type: "application/octet-stream"})
		const formData = new FormData()
		formData.append("file", blob)
		const firstBlockContentUri = yield* upload(formData, config.ipfsGatewayWrite)

		const plugins: PluginInstallationWithViem[] = []

		const spacePluginInstallItem = getSpacePluginInstallItem({
			firstBlockContentUri,
			// @HACK: Using a different upgrader from the governance plugin to work around
			// a limitation in Aragon.
			pluginUpgrader: getChecksumAddress("0x42de4E0f9CdFbBc070e25efFac78F5E5bA820853"),
			network,
		})

		plugins.push(spacePluginInstallItem)

		const personalSpacePluginItem = getPersonalSpaceGovernancePluginInstallItem({
			initialEditor: getChecksumAddress(initialEditorAddress),
			network,
		})

		plugins.push(personalSpacePluginItem)

		const createParams: CreateGeoDaoParams = {
			metadataUri: firstBlockContentUri,
			plugins,
		}

		yield* Effect.logInfo("[SPACE][deploy] Creating DAO")

		const dao = yield* Effect.tryPromise({
			try: async () => {
				const steps = createDao(createParams, getDeployParams(network), network)
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
		})

		yield* Effect.logInfo("[SPACE][deploy] Deployed DAO successfully!").pipe(
			Effect.annotateLogs({dao: dao.dao, pluginAddresses: dao.pluginAddresses}),
		)

		const waitStartTime = Date.now()

		yield* Effect.logInfo("[SPACE][deploy] Waiting for DAO to be indexed into a space").pipe(
			Effect.annotateLogs({dao: dao.dao}),
		)
		const waitResult = yield* Effect.tryPromise({
			try: async () => {
				const result = await waitForSpaceToBeIndexed(dao.dao, network)
				return result
			},
			catch: (e) => new WaitForSpaceToBeIndexedError(`Failed waiting for space to be indexed: ${e}`),
		})

		const waitEndTime = Date.now() - waitStartTime
		yield* Effect.logInfo("[SPACE][deploy] Space indexed successfully").pipe(
			Effect.annotateLogs({
				dao: dao.dao,
				pluginAddresses: dao.pluginAddresses,
				spaceId: waitResult,
			}),
		)

		return waitResult
	})
}

class TimeoutError extends Error {
	_tag = "TimeoutError"
}

const query = (daoAddress: string) => ` {
  spaces(filter: { daoAddress: { equalTo: "${getChecksumAddress(daoAddress)}" } }) {
    nodes {
      id

      spacesMetadatum {
        version {
          entityId
        }
      }
    }
  }
}`

async function waitForSpaceToBeIndexed(daoAddress: string, network: "TESTNET" | "MAINNET") {
	// @TODO: Where do we fetch?
	const endpoint = ""
	// network === "TESTNET" ? EnvironmentLive.apiEndpointTestnet : EnvironmentLiveRaw.API_ENDPOINT_MAINNET

	const graphqlFetchEffect = graphql<{
		spaces: {nodes: {id: string; spacesMetadatum: {version: {entityId: string}}}[]}
	}>({
		endpoint,
		query: query(daoAddress),
	})

	const graphqlFetchWithErrorFallbacks = Effect.gen(function* () {
		const resultOrError = yield* Effect.either(graphqlFetchEffect)

		if (Either.isLeft(resultOrError)) {
			const error = resultOrError.left

			switch (error._tag) {
				case "GraphqlRuntimeError":
					console.error(
						`Encountered runtime graphql error in waitForSpaceToBeIndexed. endpoint: ${endpoint}

            queryString: ${query(daoAddress)}
            `,
						error.message,
					)

					return null

				default:
					console.error(`${error._tag}: Unable to wait for space to be indexed, endpoint: ${endpoint}`)

					return null
			}
		}

		const maybeSpace = resultOrError.right.spaces.nodes[0]

		if (!maybeSpace) {
			yield* Effect.fail(new TimeoutError("Could not find deployed space"))
			return null
		}

		if (!maybeSpace.spacesMetadatum) {
			yield* Effect.fail(new TimeoutError("Could not find deployed space"))
			return null
		}

		return maybeSpace.id
	})

	const retried = Effect.retry(
		graphqlFetchWithErrorFallbacks,
		Schedule.exponential(100).pipe(
			Schedule.jittered,
			Schedule.compose(Schedule.elapsed),
			// Retry for 60 seconds.
			Schedule.whileOutput(Duration.lessThanOrEqualTo(Duration.seconds(60))),
		),
	)

	return await Effect.runPromise(retried)
}

async function* createDao(params: CreateGeoDaoParams, context: ContextParams, network: "TESTNET" | "MAINNET") {
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
	const pluginSetupProcessorAddress = contracts[network].PLUGIN_SETUP_PROCESSOR_ADDRESS
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

	const walletClient = getWalletClient(network)

	// We use viem as we run into unexpected "unknown account" errors when using ethers to
	// write the tx using the geo signer.
	const daoFactoryAddress = contracts[network].DAO_FACTORY_ADDRESS
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

	const publicClient = getPublicClient(network)
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

// Using viem for the dao creation requires a slightly different encoding state for our plugins.
// When using ethers the type for `data` is expected to be a Uint8Array, but when using viem and
// encodeFunctionData it expects a hex bytes string.
export interface CreateGeoDaoParams extends OmitStrict<CreateDaoParams, "plugins"> {
	plugins: PluginInstallationWithViem[]
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
	network,
}: {
	firstBlockContentUri: string
	pluginUpgrader: string
	precedessorSpace?: string
	network: "TESTNET" | "MAINNET"
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

	const spacePluginRepoAddress = contracts[network].SPACE_PLUGIN_REPO_ADDRESS

	return {
		id: spacePluginRepoAddress as `0x${string}`,
		data: encodedParams,
	}
}

export function getPersonalSpaceGovernancePluginInstallItem({
	initialEditor,
	network,
}: {
	initialEditor: string
	network: "TESTNET" | "MAINNET"
}): PluginInstallationWithViem {
	// Define the ABI for the prepareInstallation function's inputs. This comes from the
	// `personal-space-admin-build-metadata.json` in our contracts repo, not from the setup plugin's ABIs.
	const prepareInstallationInputs = [
		{
			name: "_initialEditorAddress",
			type: "address",
			internalType: "address",
			description: "The address of the first address to be granted the editor permission.",
		},
	]

	const encodedParams = encodeAbiParameters(prepareInstallationInputs, [initialEditor])

	const personalSpaceAdminPluginRepoAddress = contracts[network].PERSONAL_SPACE_ADMIN_PLUGIN_REPO_ADDRESS

	return {
		id: personalSpaceAdminPluginRepoAddress as `0x${string}`,
		data: encodedParams,
	}
}
