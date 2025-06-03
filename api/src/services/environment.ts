import {Config, Context, Effect, Option, type Redacted} from "effect"

export type IEnvironment = Readonly<{
	databaseUrl: Redacted.Redacted
	debug: boolean | null
	telemetryUrl: Redacted.Redacted | null
}>

export const make = Effect.gen(function* (_) {
	const databaseUrl = yield* _(Config.redacted("DATABASE_URL"))
	const maybeDebug = yield* _(Config.option(Config.boolean("DEBUG")))

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
		databaseUrl,
		telemetryUrl,
		debug,
	} as const
})

export class Environment extends Context.Tag("environment")<Environment, IEnvironment>() {}
export const EnvironmentLive: IEnvironment = Effect.runSync(make)
