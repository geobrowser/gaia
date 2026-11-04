import {readFileSync} from "node:fs"

export type Config = {
	kafka: {broker: string; username: string; password: string; caPem: string; topic: string; groupId: string}
	dataDir: string
	runDir: string
	sealerFile: string
	clientBin: string
	keyPath: string
	bindingPath: string
	chunk: number
	metricsPort: number
	spaceMapPath: string
	spaceMapConfigMap: string
	spacesListPath: string
	lagPollMs: number
	membership: MembershipConfig
	proposals: {enabled: boolean; topic: string; groupId: string}
}

export type MembershipConfig = {
	/** GEODB_MEMBERSHIP_DUAL_WRITE: consume `space.membership` and write members/editors into geo-db. Default off. */
	dualWrite: boolean
	/** GEODB_MEMBERSHIP_SHADOW_COMPARE: compare member/editor counts per Space, Postgres vs geo-db. Default off. */
	shadowCompare: boolean
	topic: string
	groupId: string
	compareIntervalMs: number
	/** Key the comparison reads geo-db with. */
	readerKeyPath: string
}

export const flag = (v: string | undefined): boolean => v === "1" || v?.toLowerCase() === "true"

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
		sealerFile: env.GEODB_SEALER_FILE ?? "/etc/geodb/space-map/sealer-id",
		clientBin: env.GEODB_CLIENT_BIN ?? "geodb-client",
		keyPath: required(env, "GEODB_KEY_PATH"),
		bindingPath: required(env, "GEODB_BINDING_PATH"),
		chunk,
		metricsPort: Number(env.METRICS_PORT ?? 9090),
		spaceMapPath: env.GEODB_SPACE_MAP_PATH ?? `${dataDir}/feeder/space-map.json`,
		spaceMapConfigMap: env.SPACE_MAP_CONFIGMAP ?? "geodb-space-map",
		spacesListPath: env.SPACES_LIST_PATH ?? "",
		lagPollMs: Number(env.LAG_POLL_MS ?? 15_000),
		membership: {
			dualWrite: flag(env.GEODB_MEMBERSHIP_DUAL_WRITE),
			shadowCompare: flag(env.GEODB_MEMBERSHIP_SHADOW_COMPARE),
			topic: env.KAFKA_MEMBERSHIP_TOPIC ?? `${environment}.space.membership`,
			groupId: env.KAFKA_MEMBERSHIP_GROUP_ID ?? `geodb-feeder-membership-v1-${environment}`,
			compareIntervalMs: Number(env.GEODB_MEMBERSHIP_COMPARE_INTERVAL_MS ?? 300_000),
			readerKeyPath: env.GEODB_READER_KEY_PATH ?? required(env, "GEODB_KEY_PATH"),
		},
		proposals: {
			/** GEODB_PROPOSAL_ENTITIES: consume `space.governance` and write proposal system entities. Default off. */
			enabled: flag(env.GEODB_PROPOSAL_ENTITIES),
			topic: env.KAFKA_PROPOSALS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSALS_GROUP_ID ?? `geodb-feeder-proposals-v1-${environment}`,
		},
	}
}

export const readIfExists = (path: string): string | null => {
	try {
		return readFileSync(path, "utf8")
	} catch {
		return null
	}
}
