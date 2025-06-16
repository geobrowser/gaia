import {providers} from "ethers"
import {http, createPublicClient, createWalletClient} from "viem"
import type {WalletClient} from "viem"
import {privateKeyToAccount} from "viem/accounts"
import {EnvironmentLive} from "../services/environment"
import {GEOGENESIS, TESTNET} from "./chain"

const geoAccount = privateKeyToAccount(process.env.DEPLOYER_PK as `0x${string}`)

export const getWalletClient = () => {
	return createWalletClient({
		account: geoAccount,
		chain: EnvironmentLive.chainId === "19411" ? GEOGENESIS : TESTNET,
		transport: http(EnvironmentLive.rpcEndpoint, {batch: true}),
	})
}

export const getPublicClient = () => {
	return createPublicClient({
		chain: EnvironmentLive.chainId === "19411" ? GEOGENESIS : TESTNET,
		transport: http(EnvironmentLive.rpcEndpoint, {batch: true}),
	})
}

export const getSigner = () => {
	const walletClient = getWalletClient()
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
