import {readFileSync} from "node:fs"

export type Config = {
	kafka: {broker: string; username: string; password: string; caPem: string; topic: string; groupId: string}
	dataDir: string
	runDir: string
	ctlDir: string
	clientBin: string
	keyPath: string
	bindingPath: string
	chunk: number
	metricsPort: number
	spaceMapPath: string
	spaceMapConfigMap: string
	spacesListPath: string
	lagPollMs: number
}

function required(env: NodeJS.ProcessEnv, name: string): string {
	const v = env[name]
	if (!v) throw new Error(`${name} is required`)
	return v
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
	const environment = env.ENVIRONMENT ?? "testnet"
	const dataDir = env.DATA_DIR ?? "/data"
	const chunk = Number(env.CHUNK ?? 200)
	if (!Number.isInteger(chunk) || chunk < 1) throw new Error("CHUNK must be a positive integer")
	return {
		kafka: {
			broker: required(env, "KAFKA_BROKER"),
			username: required(env, "KAFKA_USERNAME"),
			password: required(env, "KAFKA_PASSWORD"),
			caPem: required(env, "KAFKA_SSL_CA_PEM"),
			topic: env.KAFKA_TOPIC ?? `${environment}.knowledge.edits`,
			groupId: env.KAFKA_GROUP_ID ?? `geodb-feeder-v1-${environment}`,
		},
		dataDir,
		runDir: env.RUN_DIR ?? `${dataDir}/run`,
		ctlDir: env.CTL_DIR ?? `${dataDir}/ctl`,
		clientBin: env.GEODB_CLIENT_BIN ?? "geodb-client",
		keyPath: required(env, "GEODB_KEY_PATH"),
		bindingPath: required(env, "GEODB_BINDING_PATH"),
		chunk,
		metricsPort: Number(env.METRICS_PORT ?? 9090),
		spaceMapPath: env.GEODB_SPACE_MAP_PATH ?? `${dataDir}/feeder/space-map.json`,
		spaceMapConfigMap: env.SPACE_MAP_CONFIGMAP ?? "geodb-space-map",
		spacesListPath: env.SPACES_LIST_PATH ?? "",
		lagPollMs: Number(env.LAG_POLL_MS ?? 15_000),
	}
}

export const readIfExists = (path: string): string | null => {
	try {
		return readFileSync(path, "utf8")
	} catch {
		return null
	}
}
