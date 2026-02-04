import {Config, Context, Effect, Option, type Redacted} from "effect"

export type IEnvironment = Readonly<{
	databaseUrl: Redacted.Redacted
	debug: boolean | null
	ipfsKey: string
	ipfsGatewayWrite: string
}>

export const make = Effect.gen(function* (_) {
	const databaseUrl = yield* _(Config.redacted("DATABASE_URL"))
	const maybeDebug = yield* _(Config.option(Config.boolean("DEBUG")))
	const ipfsKey = yield* Config.string("IPFS_KEY")
	const ipfsGatewayWrite = yield* Config.string("IPFS_GATEWAY_WRITE")

	const debug = Option.match(maybeDebug, {
		onSome: (o) => o,
		onNone: () => null,
	})

	return {
		databaseUrl: databaseUrl,
		debug,
		ipfsKey: ipfsKey,
		ipfsGatewayWrite: ipfsGatewayWrite,
	} as const
})

export class Environment extends Context.Tag("environment")<Environment, IEnvironment>() {}
export const EnvironmentLive: IEnvironment = Effect.runSync(make)
