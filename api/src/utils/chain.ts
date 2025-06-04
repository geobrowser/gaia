import type {Chain} from "viem"
import {EnvironmentLive} from "../services/environment"

export const GEOGENESIS: Chain = {
	id: 80451, // or 80451 for mainnet
	name: "Geo Genesis",
	nativeCurrency: {
		name: "Ethereum",
		symbol: "ETH",
		decimals: 18,
	},
	rpcUrls: {
		default: {
			http: [EnvironmentLive.rpcEndpointMainnet],
		},
		public: {
			http: [EnvironmentLive.rpcEndpointMainnet],
		},
	},
}

export const TESTNET: Chain = {
	id: 19411, // or 80451 for mainnet
	name: "Geo Genesis",
	nativeCurrency: {
		name: "Ethereum",
		symbol: "ETH",
		decimals: 18,
	},
	rpcUrls: {
		default: {
			http: [EnvironmentLive.rpcEndpointTestnet],
		},
		public: {
			http: [EnvironmentLive.rpcEndpointTestnet],
		},
	},
}
