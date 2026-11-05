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
	spaceEntities: {enabled: boolean; topic: string; groupId: string}
	/**
	 * GEODB_RELATION_POSITIONS: write a relation's position into geo-db (`.position` on its `add`). Default off: a
	 * geodb-client older than ISS-587-318 refuses the key, so turn it on only once the geodb image carries it.
	 */
	relationPositions: boolean
	/**
	 * GEODB_ENTITY_TIMES: write each changed entity's createdAt / updatedAt (two reserved properties, from the edit's
	 * block time) into geo-db. Default off: it is one more `set` per touched entity per edit.
	 */
	entityTimes: boolean
	/**
	 * GEODB_RECORD_KEYS: write gaia's chain-indexed rows as natural-key records (geo-db plan 10.12) in the feeder's
	 * system Spaces, and allow the `put` intent. Default off. Needs serve at geo-db 9d97069a78 or later (which knows
	 * `put` and admits a retract of an unseen id); turn it on only after that serve is rolled. Today it moves the
	 * membership consumer (with GEODB_MEMBERSHIP_DUAL_WRITE) and the membership shadow compare to keyed ids.
	 */
	recordKeys: boolean
	/** Where the system Spaces' ConfigMap value is kept on the volume before it is published. */
	systemSpaceMapPath: string
}

export type MembershipConfig = {
	/** GEODB_MEMBERSHIP_DUAL_WRITE: consume `space.membership` and write members/editors into geo-db. Default off. */
	dualWrite: boolean
	/** GEODB_MEMBERSHIP_SHADOW_COMPARE: compare member/editor counts per Space, Postgres vs geo-db. Default off. */
	shadowCompare: boolean
	topic: string
	groupId: string
	/** The keyed consumer's own group (GEODB_RECORD_KEYS), so it replays from the start beside the legacy group. */
	keyedGroupId: string
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
		relationPositions: flag(env.GEODB_RELATION_POSITIONS),
		entityTimes: flag(env.GEODB_ENTITY_TIMES),
		recordKeys: flag(env.GEODB_RECORD_KEYS),
		systemSpaceMapPath: env.GEODB_SYSTEM_SPACE_MAP_PATH ?? `${dataDir}/feeder/system-space-map.json`,
		membership: {
			dualWrite: flag(env.GEODB_MEMBERSHIP_DUAL_WRITE),
			shadowCompare: flag(env.GEODB_MEMBERSHIP_SHADOW_COMPARE),
			topic: env.KAFKA_MEMBERSHIP_TOPIC ?? `${environment}.space.membership`,
			groupId: env.KAFKA_MEMBERSHIP_GROUP_ID ?? `geodb-feeder-membership-v1-${environment}`,
			keyedGroupId: env.KAFKA_MEMBERSHIP_KEYED_GROUP_ID ?? `geodb-feeder-membership-keyed-v1-${environment}`,
			compareIntervalMs: Number(env.GEODB_MEMBERSHIP_COMPARE_INTERVAL_MS ?? 300_000),
			readerKeyPath: env.GEODB_READER_KEY_PATH ?? required(env, "GEODB_KEY_PATH"),
		},
		proposals: {
			/** GEODB_PROPOSAL_ENTITIES: consume `space.governance` and write proposal system entities. Default off. */
			enabled: flag(env.GEODB_PROPOSAL_ENTITIES),
			topic: env.KAFKA_PROPOSALS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSALS_GROUP_ID ?? `geodb-feeder-proposals-v1-${environment}`,
		},
		spaceEntities: {
			/** GEODB_SPACE_ENTITIES: consume `space.creations` and write Space system entities. Default off. */
			enabled: flag(env.GEODB_SPACE_ENTITIES),
			topic: env.KAFKA_SPACE_CREATIONS_TOPIC ?? `${environment}.space.creations`,
			groupId: env.KAFKA_SPACE_CREATIONS_GROUP_ID ?? `geodb-feeder-space-creations-v1-${environment}`,
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
