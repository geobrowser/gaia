import {providers} from "ethers"
import {http, createPublicClient, createWalletClient} from "viem"
import type {WalletClient} from "viem"
import {privateKeyToAccount} from "viem/accounts"
import {EnvironmentLive} from "../services/environment"
import {GEOGENESIS, TESTNET} from "./chain"

const geoAccount = privateKeyToAccount(process.env.DEPLOYER_PK as `0x${string}`)

export const getWalletClient = (network: "TESTNET" | "MAINNET") => {
	const rpcEndpoint = network === "TESTNET" ? EnvironmentLive.rpcEndpointTestnet : EnvironmentLive.rpcEndpointMainnet
	return createWalletClient({
		account: geoAccount,
		chain: network === "TESTNET" ? TESTNET : GEOGENESIS,
		transport: http(rpcEndpoint, {batch: true}),
	})
}

export const getPublicClient = (network: "TESTNET" | "MAINNET") => {
	const rpcEndpoint = network === "TESTNET" ? EnvironmentLive.rpcEndpointTestnet : EnvironmentLive.rpcEndpointMainnet

	return createPublicClient({
		chain: GEOGENESIS,
		transport: http(rpcEndpoint, {batch: true}),
	})
}

export const getSigner = (network: "TESTNET" | "MAINNET") => {
	const walletClient = getWalletClient(network)
	return walletClientToSigner(walletClient)
}

function walletClientToSigner(walletClient: WalletClient) {
	const {account, chain, transport} = walletClient

	if (!chain) return

	const network = {
		chainId: chain.id,
		name: chain.name,
		ensAddress: chain.contracts?.ensRegistry?.address,
	}
	const provider = new providers.Web3Provider(transport, network)
	const signer = provider.getSigner(account?.address)
	return signer
}
