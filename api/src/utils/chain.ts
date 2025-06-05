import type {Chain} from "viem"
import {EnvironmentLive} from "../services/environment"

export const GEOGENESIS: Chain = {
	id: 80451, // or 19411 for testnet
	name: "Geo Genesis",
	nativeCurrency: {
		name: "The Graph",
		symbol: "GRT",
		decimals: 18,
	},
	rpcUrls: {
		default: {
			http: [EnvironmentLive.rpcEndpoint],
		},
		public: {
			http: [EnvironmentLive.rpcEndpoint],
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
			http: [EnvironmentLive.rpcEndpoint],
		},
		public: {
			http: [EnvironmentLive.rpcEndpoint],
		},
	},
}
