import {Graph, getChecksumAddress, type Op, SystemIds} from "@graphprotocol/grc-20"
import {EditProposal} from "@graphprotocol/grc-20/proto"
import {Effect} from "effect"
import type {ethers} from "ethers"
import {encodeAbiParameters} from "viem"
import {Environment} from "../services/environment"
import {upload} from "../services/ipfs"
import {type CreateGeoDaoParams, deployAndValidateDao} from "./deploy-dao"
import {
	contracts,
	generateEditFormData,
	getChecksumAddresses,
	getSpacePluginInstallItem,
	type PluginInstallationWithViem,
	pctToRatio,
} from "./std"

enum VotingMode {
	STANDARD = 1,
	EARLY_EXECUTION = 2,
	VOTE_REPLACEMENT = 3,
}

interface DeployArgs {
	spaceName: string
	initialEditorAddresses: string[]
	spaceEntityId?: string
	ops?: Op[]
}

export function deployPublicSpace(args: DeployArgs) {
	return Effect.gen(function* () {
		const config = yield* Environment
		yield* Effect.logInfo(`[SPACE][deploy] Deploying PUBLIC space for ${config.chainId}`)

		const initialEditorAddresses = getChecksumAddresses(args.initialEditorAddresses)
		const firstEditor = initialEditorAddresses[0] as `0x${string}`

		const entityOp = Graph.createEntity({
			id: args.spaceEntityId,
			name: args.spaceName,
			types: [SystemIds.SPACE_TYPE], // What if the space type is already set?
		})

		const initialContent = EditProposal.encode({
			name: args.spaceName,
			author: firstEditor,
			ops: [...entityOp.ops, ...(args.ops ? args.ops : [])],
		})

		yield* Effect.logInfo("[SPACE][deploy] Uploading EDIT to IPFS")
		const formData = generateEditFormData(initialContent)
		const firstBlockContentUri = yield* upload(formData, config.ipfsGatewayWrite).pipe(
			Effect.withSpan("deployPublicSpace.upload"),
			Effect.annotateSpans({
				...args,
			}),
		)

		const plugins: PluginInstallationWithViem[] = []

		const spacePluginInstallItem = getSpacePluginInstallItem({
			firstBlockContentUri,
			// @HACK: Using a different upgrader from the governance plugin to work around
			// a limitation in Aragon.
			pluginUpgrader: getChecksumAddress("0x42de4E0f9CdFbBc070e25efFac78F5E5bA820853"),
		})

		plugins.push(spacePluginInstallItem)

		const governancePluginConfig: Parameters<typeof getGovernancePluginInstallItem>[0] = {
			votingSettings: {
				votingMode: VotingMode.EARLY_EXECUTION,
				supportThreshold: pctToRatio(50),
				duration: BigInt(60 * 60 * 24), // 24 hours
			},
			memberAccessProposalDuration: BigInt(60 * 60 * 4), // 4 hours
			initialEditors: initialEditorAddresses,
			pluginUpgrader: firstEditor,
		}

		const governancePluginInstallItem = getGovernancePluginInstallItem(governancePluginConfig)
		plugins.push(governancePluginInstallItem)

		const createParams: CreateGeoDaoParams = {
			metadataUri: firstBlockContentUri,
			plugins,
		}

		yield* Effect.logInfo("[SPACE][deploy] Creating DAO")

		return yield* deployAndValidateDao(createParams).pipe(
			Effect.withSpan("deployPublicSpace.deployAndValidateDao"),
			Effect.annotateSpans({...createParams, ...args}),
		)
	})
}

function getGovernancePluginInstallItem(params: {
	votingSettings: {
		votingMode: VotingMode
		supportThreshold: ethers.BigNumber
		duration: bigint
	}
	initialEditors: `0x${string}`[]
	memberAccessProposalDuration: bigint
	pluginUpgrader: `0x${string}`
}): PluginInstallationWithViem {
	// From `encodeInstallationParams`
	const prepareInstallationInputs = [
		{
			components: [
				{
					internalType: "enum MajorityVotingBase.VotingMode",
					name: "votingMode",
					type: "uint8",
				},
				{
					internalType: "uint32",
					name: "supportThreshold",
					type: "uint32",
				},
				{
					internalType: "uint64",
					name: "duration",
					type: "uint64",
				},
			],
			internalType: "struct MajorityVotingBase.VotingSettings",
			name: "_votingSettings",
			type: "tuple",
		},
		{
			internalType: "address[]",
			name: "_initialEditors",
			type: "address[]",
		},
		{
			internalType: "uint64",
			name: "_memberAccessProposalDuration",
			type: "uint64",
		},
		{
			internalType: "address",
			name: "_pluginUpgrader",
			type: "address",
		},
	]

	const encodedParams = encodeAbiParameters(prepareInstallationInputs, [
		params.votingSettings,
		params.initialEditors,
		params.memberAccessProposalDuration,
		params.pluginUpgrader,
	])

	return {
		id: contracts.GOVERNANCE_PLUGIN_REPO_ADDRESS as `0x${string}`,
		data: encodedParams,
	}
}
