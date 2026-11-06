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
	/** GEODB_VOTES (with GEODB_RECORD_KEYS): keyed user votes from `curation.votes` (votesKeyed.ts). */
	votes: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_PROPOSAL_VOTES (with GEODB_RECORD_KEYS): keyed proposal votes from `space.governance` (proposalVotesKeyed.ts). */
	proposalVotes: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_PROPOSAL_ACTIONS (with GEODB_RECORD_KEYS): keyed proposal actions from `space.governance` (proposalActionsKeyed.ts). */
	proposalActions: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_SUBSPACES (with GEODB_RECORD_KEYS): keyed subspaces and subspace topics from `space.trust.extensions` (subspacesKeyed.ts). */
	subspaces: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_SPACE_VOTING_SETTINGS (with GEODB_RECORD_KEYS): keyed voting settings from `space.governance` (spaceVotingSettingsKeyed.ts). */
	spaceVotingSettings: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_FOR_YOU_CONFIG (with GEODB_RECORD_KEYS): the for_you_config row as a keyed record, by backfill only (forYouConfigKeyed.ts). */
	forYouConfig: {enabled: boolean}
	/** GEODB_ENTITY_RANKING_CONFIG (with GEODB_RECORD_KEYS): the entity_ranking_config row as a keyed record, by backfill only (entityRankingConfigKeyed.ts). */
	entityRankingConfig: {enabled: boolean}
	/** GEODB_INTEREST_CONFIG (with GEODB_RECORD_KEYS): interest_config and interest_signal_weights as keyed records, by backfill only (interestConfigKeyed.ts). */
	interestConfig: {enabled: boolean}
	/** GEODB_FEED_EXPERIMENTS (with GEODB_RECORD_KEYS): feed_experiments as keyed records, by backfill only (feedExperimentsKeyed.ts). */
	feedExperiments: {enabled: boolean}
	/** GEODB_FEED_EXPERIMENT_MEMBERS (with GEODB_RECORD_KEYS): feed_experiment_members as keyed records, by backfill only (feedExperimentMembersKeyed.ts). */
	feedExperimentMembers: {enabled: boolean}
	/** GEODB_PROPOSAL_VERSIONS (with GEODB_RECORD_KEYS): keyed proposal versions from `space.governance` (proposalVersionsKeyed.ts). */
	proposalVersions: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_PROPOSAL_STATUS (with GEODB_RECORD_KEYS): keyed executed_at / unexecutable_at facts from `space.governance` (proposalStatusKeyed.ts). */
	proposalStatus: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_RANKS (with GEODB_RECORD_KEYS): keyed ranking blocks, rankings and ranking items from `knowledge.edits` (ranksKeyed.ts). */
	ranks: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_SPACE_TOPICS (with GEODB_RECORD_KEYS): keyed `spaces.topic_id` from `space.topics` (spaceTopicsKeyed.ts). */
	spaceTopics: {enabled: boolean; topic: string; groupId: string}
	/** GEODB_SPACE_ADDRESSES (with GEODB_RECORD_KEYS): keyed `spaces.address` from `space.creations` (spaceAddressesKeyed.ts). */
	spaceAddresses: {enabled: boolean; topic: string; groupId: string}
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
	/**
	 * The publisher's copy of the `system-spaces.json` ConfigMap value. Not the durable record: that is
	 * `<DATA_DIR>/feeder/system-spaces.json` (systemSpaces.ts), which this is rendered from and can be rebuilt from.
	 */
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

/**
 * Every Kafka consumer group the feeder can run, whatever its flags. The backfill Jobs refuse while any has members:
 * they share the feeder's state volume and system-space record. Add a new consumer's group here.
 */
export const feederGroupIds = (c: Config): string[] => [
	c.kafka.groupId,
	c.membership.groupId,
	c.membership.keyedGroupId,
	c.proposals.groupId,
	c.spaceEntities.groupId,
	c.votes.groupId,
	c.proposalVotes.groupId,
	c.proposalActions.groupId,
	c.subspaces.groupId,
	c.spaceVotingSettings.groupId,
	c.spaceTopics.groupId,
	c.spaceAddresses.groupId,
	c.proposalVersions.groupId,
	c.proposalStatus.groupId,
	c.ranks.groupId,
]

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
		systemSpaceMapPath: env.GEODB_SYSTEM_SPACE_MAP_PATH ?? `${dataDir}/feeder/system-spaces.published.json`,
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
		votes: {
			/**
			 * GEODB_VOTES: consume `curation.votes` and write keyed user-vote records (votesKeyed.ts) into the `votes`
			 * system Space. Takes effect only with GEODB_RECORD_KEYS (votesKeyedEnabled), and only after the votes
			 * backfill (RUNBOOK §2c-ter). Default off.
			 */
			enabled: flag(env.GEODB_VOTES),
			topic: env.KAFKA_VOTES_TOPIC ?? `${environment}.curation.votes`,
			groupId: env.KAFKA_VOTES_GROUP_ID ?? `geodb-feeder-votes-keyed-v1-${environment}`,
		},
		proposalVotes: {
			/**
			 * GEODB_PROPOSAL_VOTES: consume the PROPOSAL_VOTED events of `space.governance` and write keyed
			 * proposal-vote records (proposalVotesKeyed.ts) into the `votes` system Space. Takes effect only with
			 * GEODB_RECORD_KEYS (proposalVotesKeyedEnabled), and only after the proposal-votes backfill (RUNBOOK
			 * §2c-quater). Its own group: the proposals consumer's group on the same topic is untouched. Default off.
			 */
			enabled: flag(env.GEODB_PROPOSAL_VOTES),
			topic: env.KAFKA_PROPOSAL_VOTES_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSAL_VOTES_GROUP_ID ?? `geodb-feeder-proposal-votes-keyed-v1-${environment}`,
		},
		proposalActions: {
			/**
			 * GEODB_PROPOSAL_ACTIONS: consume the PROPOSAL_CREATED and PROPOSAL_UPDATED events of `space.governance`
			 * and write keyed proposal-action records (proposalActionsKeyed.ts) into the `governance` system Space.
			 * Takes effect only with GEODB_RECORD_KEYS (proposalActionsKeyedEnabled), and only after the
			 * proposal-actions backfill (RUNBOOK §2c-quinquies). Its own group. Default off.
			 */
			enabled: flag(env.GEODB_PROPOSAL_ACTIONS),
			topic: env.KAFKA_PROPOSAL_ACTIONS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSAL_ACTIONS_GROUP_ID ?? `geodb-feeder-proposal-actions-keyed-v1-${environment}`,
		},
		subspaces: {
			/**
			 * GEODB_SUBSPACES: consume `space.trust.extensions` and write keyed subspace and subspace-topic edges
			 * (subspacesKeyed.ts) into the `governance` system Space. Takes effect only with GEODB_RECORD_KEYS
			 * (subspacesKeyedEnabled), and only after the subspaces backfill (RUNBOOK §2c-sexies). Its own group. Default off.
			 */
			enabled: flag(env.GEODB_SUBSPACES),
			topic: env.KAFKA_SUBSPACES_TOPIC ?? `${environment}.space.trust.extensions`,
			groupId: env.KAFKA_SUBSPACES_GROUP_ID ?? `geodb-feeder-subspaces-keyed-v1-${environment}`,
		},
		spaceVotingSettings: {
			/**
			 * GEODB_SPACE_VOTING_SETTINGS: consume the VOTING_SETTINGS_UPDATED events of `space.governance` and write
			 * keyed voting-settings records (spaceVotingSettingsKeyed.ts) into the `governance` system Space. Takes effect
			 * only with GEODB_RECORD_KEYS (spaceVotingSettingsKeyedEnabled), and only after the backfill (RUNBOOK
			 * §2c-septies). Its own group. Default off.
			 */
			enabled: flag(env.GEODB_SPACE_VOTING_SETTINGS),
			topic: env.KAFKA_SPACE_VOTING_SETTINGS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_SPACE_VOTING_SETTINGS_GROUP_ID ?? `geodb-feeder-space-voting-settings-keyed-v1-${environment}`,
		},
		/** GEODB_FOR_YOU_CONFIG: lets the for-you-config backfill Job write the record. No consumer. Default off. */
		forYouConfig: {enabled: flag(env.GEODB_FOR_YOU_CONFIG)},
		/** GEODB_ENTITY_RANKING_CONFIG: lets the entity-ranking-config backfill Job write the record. No consumer. Default off. */
		entityRankingConfig: {enabled: flag(env.GEODB_ENTITY_RANKING_CONFIG)},
		/** GEODB_INTEREST_CONFIG: lets the interest-config backfill Job write the records. No consumer. Default off. */
		interestConfig: {enabled: flag(env.GEODB_INTEREST_CONFIG)},
		/** GEODB_FEED_EXPERIMENTS: lets the feed-experiments backfill Job write the records. No consumer. Default off. */
		feedExperiments: {enabled: flag(env.GEODB_FEED_EXPERIMENTS)},
		/** GEODB_FEED_EXPERIMENT_MEMBERS: lets the feed-experiment-members backfill Job write the records. No consumer. Default off. */
		feedExperimentMembers: {enabled: flag(env.GEODB_FEED_EXPERIMENT_MEMBERS)},
		proposalVersions: {
			/**
			 * GEODB_PROPOSAL_VERSIONS: consume the PROPOSAL_CREATED and PROPOSAL_UPDATED events of `space.governance`
			 * and write keyed proposal-version records (proposalVersionsKeyed.ts) into the `governance` system Space.
			 * Takes effect only with GEODB_RECORD_KEYS (proposalVersionsKeyedEnabled), and only after the
			 * proposal-versions backfill (RUNBOOK §2c-octies). Its own group. Default off.
			 */
			enabled: flag(env.GEODB_PROPOSAL_VERSIONS),
			topic: env.KAFKA_PROPOSAL_VERSIONS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSAL_VERSIONS_GROUP_ID ?? `geodb-feeder-proposal-versions-keyed-v1-${environment}`,
		},
		proposalStatus: {
			/**
			 * GEODB_PROPOSAL_STATUS: consume the PROPOSAL_EXECUTED events of `space.governance` and write keyed
			 * proposal-status records (proposalStatusKeyed.ts) into the `governance` system Space. Takes effect only with
			 * GEODB_RECORD_KEYS (proposalStatusKeyedEnabled), and only after the backfill (RUNBOOK §2c-novies). Its own
			 * group. Default off.
			 */
			enabled: flag(env.GEODB_PROPOSAL_STATUS),
			topic: env.KAFKA_PROPOSAL_STATUS_TOPIC ?? `${environment}.space.governance`,
			groupId: env.KAFKA_PROPOSAL_STATUS_GROUP_ID ?? `geodb-feeder-proposal-status-keyed-v1-${environment}`,
		},
		ranks: {
			/**
			 * GEODB_RANKS: consume `knowledge.edits` on a group of its own and write the ranking-indexer's three tables
			 * (ranks.ranking_blocks, rankings, ranking_items) as keyed records (ranksKeyed.ts) into the `votes` system
			 * Space. Takes effect only with GEODB_RECORD_KEYS (ranksKeyedEnabled), and only after the backfill
			 * (RUNBOOK §2c-decies). Default off.
			 */
			enabled: flag(env.GEODB_RANKS),
			topic: env.KAFKA_RANKS_TOPIC ?? `${environment}.knowledge.edits`,
			groupId: env.KAFKA_RANKS_GROUP_ID ?? `geodb-feeder-ranks-keyed-v1-${environment}`,
		},
		spaceTopics: {
			/**
			 * GEODB_SPACE_TOPICS: consume the TOPIC_DECLARED and TOPIC_REMOVED events of `space.topics` and write each
			 * Space's current topic (spaceTopicsKeyed.ts) into the `governance` system Space. Takes effect only with
			 * GEODB_RECORD_KEYS (spaceTopicsKeyedEnabled), and only after the backfill (RUNBOOK §2c-undecies). Its own
			 * group. Default off.
			 */
			enabled: flag(env.GEODB_SPACE_TOPICS),
			topic: env.KAFKA_SPACE_TOPICS_TOPIC ?? `${environment}.space.topics`,
			groupId: env.KAFKA_SPACE_TOPICS_GROUP_ID ?? `geodb-feeder-space-topics-keyed-v1-${environment}`,
		},
		spaceAddresses: {
			/**
			 * GEODB_SPACE_ADDRESSES: consume the SPACE_ADDRESS_UPDATED and HermesCreateSpace messages of `space.creations`
			 * and write each Space's current address (spaceAddressesKeyed.ts) into the `governance` system Space. Takes
			 * effect only with GEODB_RECORD_KEYS (spaceAddressesKeyedEnabled), and only after the backfill (RUNBOOK
			 * §2c-duodecies). Its own group (a new one: its Kafka ACL must allow it). Default off.
			 */
			enabled: flag(env.GEODB_SPACE_ADDRESSES),
			topic: env.KAFKA_SPACE_ADDRESSES_TOPIC ?? `${environment}.space.creations`,
			groupId: env.KAFKA_SPACE_ADDRESSES_GROUP_ID ?? `geodb-feeder-space-addresses-keyed-v1-${environment}`,
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
