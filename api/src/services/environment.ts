import {Config, Context, Effect, Option, type Redacted} from "effect"

export type IEnvironment = Readonly<{
	databaseUrl: Redacted.Redacted
	debug: boolean | null
	telemetryUrl: Redacted.Redacted | null
	ipfsKey: string
	ipfsGatewayWrite: string
	ipfsGatewayRead: string
	rpcEndpointTestnet: string
	rpcEndpointMainnet: string
}>

export const make = Effect.gen(function* (_) {
	const DATABASE_URL = yield* _(Config.redacted("DATABASE_URL"))
	const maybeDebug = yield* _(Config.option(Config.boolean("DEBUG")))
	const IPFS_KEY = yield* Config.string("IPFS_KEY")
	const IPFS_GATEWAY_WRITE = yield* Config.string("IPFS_GATEWAY_WRITE")
	const IPFS_GATEWAY_READ = yield* Config.string("IPFS_GATEWAY_READ")
	const RPC_ENDPOINT_TESTNET = yield* Config.string("RPC_ENDPOINT_TESTNET")
	const RPC_ENDPOINT_MAINNET = yield* Config.string("RPC_ENDPOINT_MAINNET")

	const maybeTelemetryUrl = yield* _(Config.option(Config.redacted("TELEMETRY_URL")))
	const telemetryUrl = Option.match(maybeTelemetryUrl, {
		onSome: (o) => o,
		onNone: () => null,
	})
	const debug = Option.match(maybeDebug, {
		onSome: (o) => o,
		onNone: () => null,
	})

	return {
		databaseUrl: DATABASE_URL,
		telemetryUrl,
		debug,
		ipfsKey: IPFS_KEY,
		ipfsGatewayWrite: IPFS_GATEWAY_WRITE,
		ipfsGatewayRead: IPFS_GATEWAY_READ,
		rpcEndpointTestnet: RPC_ENDPOINT_TESTNET,
		rpcEndpointMainnet: RPC_ENDPOINT_MAINNET,
	} as const
})

export class Environment extends Context.Tag("environment")<Environment, IEnvironment>() {}
export const EnvironmentLive: IEnvironment = Effect.runSync(make)
