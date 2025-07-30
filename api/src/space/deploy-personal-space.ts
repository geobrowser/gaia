import {Graph, getChecksumAddress, type Op, SystemIds} from "@graphprotocol/grc-20"
import {EditProposal} from "@graphprotocol/grc-20/proto"
import {Effect} from "effect"
import {encodeAbiParameters} from "viem"
import {Environment} from "../services/environment"
import {upload} from "../services/ipfs"
import {type CreateGeoDaoParams, deployAndValidateDao} from "./deploy-dao"
import {contracts, generateEditFormData, getSpacePluginInstallItem, type PluginInstallationWithViem} from "./std"

interface DeployArgs {
	spaceName: string
	initialEditorAddress: string
	spaceEntityId?: string
	ops?: Op[]
}

export function deployPersonalSpace(args: DeployArgs) {
	return Effect.gen(function* () {
		const config = yield* Environment
		yield* Effect.logInfo(`[SPACE][deploy] Deploying PERSONAL space for ${config.chainId}`)
		const initialEditorAddress = getChecksumAddress(args.initialEditorAddress)

		const entityOp = Graph.createEntity({
			id: args.spaceEntityId,
			name: args.spaceName,
			types: [SystemIds.SPACE_TYPE], // What if the space type is already set?
		})

		const initialContent = EditProposal.encode({
			name: args.spaceName,
			author: initialEditorAddress,
			ops: [...entityOp.ops, ...(args.ops ? args.ops : [])],
		})

		yield* Effect.logInfo("[SPACE][deploy] Uploading EDIT to IPFS")

		const formData = generateEditFormData(initialContent)

		const firstBlockContentUri = yield* upload(formData, config.ipfsGatewayWrite).pipe(
			Effect.withSpan("deployPersonalSpace.upload"),
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

		const personalSpacePluginItem = getPersonalSpaceGovernancePluginInstallItem({
			initialEditor: getChecksumAddress(initialEditorAddress),
		})

		plugins.push(personalSpacePluginItem)

		const createParams: CreateGeoDaoParams = {
			metadataUri: firstBlockContentUri,
			plugins,
		}

		yield* Effect.logInfo("[SPACE][deploy] Creating DAO")

		return yield* deployAndValidateDao(createParams).pipe(
			Effect.withSpan("deployPersonalSpace.deployAndValidateDao"),
			Effect.annotateSpans({...createParams, ...args}),
		)
	})
}

function getPersonalSpaceGovernancePluginInstallItem({
	initialEditor,
}: {
	initialEditor: string
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

	const personalSpaceAdminPluginRepoAddress = contracts.PERSONAL_SPACE_ADMIN_PLUGIN_REPO_ADDRESS

	return {
		id: personalSpaceAdminPluginRepoAddress as `0x${string}`,
		data: encodedParams,
	}
}
