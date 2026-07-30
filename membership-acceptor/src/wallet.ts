/**
 * Smart-wallet setup — a gas-sponsored smart account.
 *
 * The acceptor votes via SpaceRegistry.enter() with `_fromSpaceId` = its own
 * personal space; the protocol requires msg.sender to be that space's account, so
 * the signer must be the smart account that owns the acceptor's personal space.
 * Ported from proposal-executor/src/execute.ts (de-Effect-ified).
 *
 * Two paths, by chain:
 *   19411 / 80451 — Safe smart account + Pimlico paymaster (original).
 *   55516         — EIP-7702 Kernel account + ZeroDev paymaster. That chain has
 *                   no Safe infra at all, but ships Kernel v0.3.3 + EntryPoint
 *                   v0.7 by default. Mirrors proposal-executor's
 *                   `createSmartWallet` (execute.ts:141) so the two stay
 *                   comparable.
 *
 * IMPORTANT identity difference: under EIP-7702 the account address IS the raw
 * EOA derived from `privateKey`, whereas the Safe path derives a separate proxy
 * address. So on 55516 the acceptor acts as its EOA, and that EOA — not a Safe —
 * must own ACCEPTOR_SPACE_ID and hold editor rights in the auto-accept spaces.
 */

import {
	createKernelAccount,
	createKernelAccountClient,
	createZeroDevPaymasterClient,
	getUserOperationGasPrice,
} from "@zerodev/sdk"
import {getEntryPoint, KERNEL_V3_3} from "@zerodev/sdk/constants"
import {createSmartAccountClient, type SmartAccountClient} from "permissionless"
import {toSafeSmartAccount} from "permissionless/accounts"
import {createPimlicoClient} from "permissionless/clients/pimlico"
import {type Address, type Chain, createPublicClient, http} from "viem"
import {entryPoint07Address} from "viem/account-abstraction"
import {privateKeyToAccount} from "viem/accounts"

import {getChain, type SupportedChainId, TESTNET_SAFE_ADDRESSES} from "./contracts.js"

export interface SmartWallet {
	readonly smartAccountClient: SmartAccountClient
	readonly chain: Chain
	/** The account the acceptor votes as: a Safe proxy on 19411/80451, the EOA itself on 55516. */
	readonly safeAddress: Address
}

export interface WalletConfig {
	privateKey: `0x${string}`
	/** Pimlico bundler/paymaster key. Unused (and meaningless) on 55516. */
	pimlicoApiKey: string
	rpcUrl: string
	chainId: SupportedChainId
	/** ZeroDev bundler+paymaster endpoint. Required when chainId is 55516, ignored otherwise. */
	zerodevSponsorshipRpcUrl?: string
}

/** Create a gas-sponsored smart account. Called once at startup. */
export async function createSmartWallet(config: WalletConfig): Promise<SmartWallet> {
	const chain = getChain(config.chainId)

	const publicClient = createPublicClient({chain, transport: http(config.rpcUrl)})
	const owner = privateKeyToAccount(config.privateKey)

	if (config.chainId === 55516) {
		if (!config.zerodevSponsorshipRpcUrl) {
			throw new Error("ZERODEV_SPONSORSHIP_RPC_URL is required when CHAIN_ID is 55516")
		}

		const entryPoint = getEntryPoint("0.7")
		const kernelAccount = await createKernelAccount(publicClient, {
			eip7702Account: owner,
			entryPoint,
			kernelVersion: KERNEL_V3_3,
		})

		const bundlerTransport = http(config.zerodevSponsorshipRpcUrl)
		const paymasterClient = createZeroDevPaymasterClient({chain, transport: bundlerTransport})

		const smartAccountClient = createKernelAccountClient({
			account: kernelAccount,
			chain,
			client: publicClient,
			bundlerTransport,
			paymaster: {
				getPaymasterStubData: (userOperation) =>
					paymasterClient.sponsorUserOperation({userOperation, shouldConsume: false}),
				getPaymasterData: (userOperation) => paymasterClient.sponsorUserOperation({userOperation}),
			},
			userOperation: {
				estimateFeesPerGas: async ({bundlerClient}) => getUserOperationGasPrice(bundlerClient),
			},
		})

		return {
			// KernelAccountClient and permissionless's SmartAccountClient both expose
			// the `.account` / `.sendTransaction(...)` shape the vote path uses — the
			// only shape SmartWallet depends on here.
			smartAccountClient: smartAccountClient as unknown as SmartAccountClient,
			chain,
			safeAddress: kernelAccount.address,
		}
	}

	const bundlerUrl = `https://api.pimlico.io/v2/${config.chainId}/rpc?apikey=${config.pimlicoApiKey}`

	const safeAccount = await toSafeSmartAccount({
		client: publicClient,
		owners: [owner],
		entryPoint: {address: entryPoint07Address, version: "0.7" as const},
		version: "1.4.1" as const,
		...(config.chainId === 19411 ? TESTNET_SAFE_ADDRESSES : {}),
	})

	const bundlerTransport = http(bundlerUrl)
	const paymasterClient = createPimlicoClient({
		transport: bundlerTransport,
		chain,
		entryPoint: {address: entryPoint07Address, version: "0.7"},
	})

	const smartAccountClient = createSmartAccountClient({
		chain,
		account: safeAccount,
		paymaster: paymasterClient,
		bundlerTransport,
		userOperation: {
			estimateFeesPerGas: async () => (await paymasterClient.getUserOperationGasPrice()).fast,
		},
	})

	return {smartAccountClient, chain, safeAddress: safeAccount.address}
}
