import {Config, Context, Effect, Option, type Redacted} from "effect"

export type IEnvironment = Readonly<{
	databaseUrl: Redacted.Redacted
	debug: boolean | null
	telemetryToken: Redacted.Redacted | null
	ipfsKey: string
	ipfsGatewayWrite: string
	rpcEndpoint: string
	chainId: "80451" | "19411"
}>

export const make = Effect.gen(function* (_) {
	const databaseUrl = yield* _(Config.redacted("DATABASE_URL"))
	const maybeDebug = yield* _(Config.option(Config.boolean("DEBUG")))
	const ipfsKey = yield* Config.string("IPFS_KEY")
	const ipfsGatewayWrite = yield* Config.string("IPFS_GATEWAY_WRITE")
	const rpcEndpoint = yield* Config.string("RPC_ENDPOINT")
	const chainId = yield* Config.string("CHAIN_ID")

	if (chainId !== "19411" && chainId !== "80451") {
		throw new Error(`Invalid configuration for chain id. Expected 19411 or 80451. Got ${chainId}`)
	}

	const maybeTelemetryToken = yield* Config.option(Config.redacted("TELEMETRY_TOKEN"))

	const telemetryToken = Option.match(maybeTelemetryToken, {
		onSome: (o) => o,
		onNone: () => null,
	})
	const debug = Option.match(maybeDebug, {
		onSome: (o) => o,
		onNone: () => null,
	})

	return {
		chainId,
		databaseUrl: databaseUrl,
		telemetryToken,
		debug,
		ipfsKey: ipfsKey,
		ipfsGatewayWrite: ipfsGatewayWrite,
		rpcEndpoint: rpcEndpoint,
	} as const
})

export class Environment extends Context.Tag("environment")<Environment, IEnvironment>() {}
export const EnvironmentLive: IEnvironment = Effect.runSync(make)
