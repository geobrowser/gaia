/**
 * Smart-wallet setup — a gas-sponsored Safe smart account (Pimlico paymaster).
 *
 * The acceptor votes via SpaceRegistry.enter() with `_fromSpaceId` = its own
 * personal space; the protocol requires msg.sender to be that space's account, so
 * the signer must be the Safe smart account that owns the acceptor's personal
 * space. Ported from proposal-executor/src/execute.ts (de-Effect-ified).
 */

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
	readonly safeAddress: Address
}

export interface WalletConfig {
	privateKey: `0x${string}`
	pimlicoApiKey: string
	rpcUrl: string
	chainId: SupportedChainId
}

/** Create a gas-sponsored Safe smart account. Called once at startup. */
export async function createSmartWallet(config: WalletConfig): Promise<SmartWallet> {
	const chain = getChain(config.chainId)
	const bundlerUrl = `https://api.pimlico.io/v2/${config.chainId}/rpc?apikey=${config.pimlicoApiKey}`

	const publicClient = createPublicClient({chain, transport: http(config.rpcUrl)})
	const owner = privateKeyToAccount(config.privateKey)

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
