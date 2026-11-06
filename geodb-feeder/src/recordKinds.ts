/**
 * Pinned record kinds and their system Spaces (geo-db plan 10.12 K2 and K4; plan 10.11 GAP-7, GAP-12).
 *
 * A kind is the 16-byte first field of a `record.key` preimage: one per gaia table (row family), pinned like a `sys`
 * atom. Each was minted once as a random UUID v4 (2026-11-05) and written here; it never changes, because every
 * keyed id of that table is derived from it, and a changed kind is a second copy of the table under new ids. A
 * random id, rather than one hashed from the table's name, cannot collide with any gaia or geo-db id and makes the
 * pin the only definition: nothing recomputes it, so nothing can recompute it differently.
 *
 * Each kind lives in one system Space per record family (K4): `governance` (who may do what in a Space and what
 * its proposals carry), `votes` (proposal and user votes) and `config` (operator-edited settings). A table moves
 * to geo-db by writing its rows in its family's Space; Spaces are not minted per table or per gaia Space, so the
 * three families cost at most three Spaces against the serve's cap.
 *
 * Keep this file additive. A kind for a table not listed yet is minted (`crypto.randomUUID()`, dashes removed)
 * and appended with its family; the test in `__tests__/recordKinds.test.ts` pins every value.
 */
export type SystemFamily = "governance" | "votes" | "config"

export const SYSTEM_FAMILIES: readonly SystemFamily[] = ["governance", "votes", "config"]

export type RecordKind = {id: string; family: SystemFamily; table: string}

export const RECORD_KINDS = {
	/** `members` and `editors`: an edge (Space entity, role property, member entity); the role is a key part. */
	membership: {id: "561407a0d813435fb6ed474be1a96209", family: "governance", table: "members, editors"},
	/** `subspaces`: an edge (parent Space entity, subspace property, child Space entity). */
	subspace: {id: "de1a992f8c2840c5bd185fdfa6aa28e9", family: "governance", table: "subspaces"},
	/** `subspace_topics`: an edge (Space entity, topic property, topic entity). */
	subspaceTopic: {id: "e90b36c75cbf4239a6add0ae26dd939a", family: "governance", table: "subspace_topics"},
	/** `proposal_actions`: a record keyed (proposal, action index). */
	proposalAction: {id: "22335b74e17a4c9e867d4ac2a32c3a4a", family: "governance", table: "proposal_actions"},
	/** `proposal_versions`: a record keyed (proposal, version-stamp block). The ordinal is not a key part. */
	proposalVersion: {id: "6863abef6664450eb135a182c6ff1715", family: "governance", table: "proposal_versions"},
	/** A settings escalation of a proposal version (`PROPOSAL_SETTINGS_UPDATED`): a record keyed (proposal, event block). */
	proposalEscalation: {id: "69352153d76946afa5d926c13c3613e9", family: "governance", table: "proposal_versions (settings escalation)"},
	/** `space_voting_settings`: a record keyed (space), mutable columns written by `put`. */
	spaceVotingSettings: {id: "7767f888a9a240b18275a7832363f4d9", family: "governance", table: "space_voting_settings"},
	/** `spaces.topic_id`: a record keyed (space); its one mutable column is the Space's current topic, written by `put`. */
	spaceTopic: {id: "c74881c2d1d24ae5ad1e34d5c1c0535b", family: "governance", table: "spaces (topic_id)"},
	/** `spaces.address`: a record keyed (space); its one mutable column is the Space's current address, written by `put`. */
	spaceAddress: {id: "44433f0b01fd46a18317285f2236fc93", family: "governance", table: "spaces (address)"},
	/** `proposals.executed_at` / `unexecutable_at`: a record keyed (proposal). The status itself is computed, not stored. */
	proposalStatus: {id: "cf398f45b2c74002bb13e5416fbc7027", family: "governance", table: "proposals (executed_at, unexecutable_at)"},
	/** `ranking_blocks`: a record keyed (block, space), its primary key. Mutable columns written by `put`. */
	rankingBlock: {id: "ce9b2464e65c450a975d3a427baaf4a2", family: "votes", table: "ranks.ranking_blocks"},
	/** `rankings`: a record keyed (ranking, space), its primary key. */
	ranking: {id: "ff8283aec584455a80198d6d80f0f4fc", family: "votes", table: "ranks.rankings"},
	/** `ranking_items`: a record keyed (ranking, entity, space), its primary key. Replaced as a whole ranking by generation. */
	rankingItem: {id: "f8ad2041f5a34f4580a1ba3ca426730f", family: "votes", table: "ranks.ranking_items"},
	/** `proposal_votes`: a record keyed (proposal, proposal version, voter), its primary key. */
	proposalVote: {id: "ca89a877762c4883a3640b5455a8f0e2", family: "votes", table: "proposal_votes"},
	/** `user_votes`: a record keyed (user, object, object type, space, kind). */
	userVote: {id: "1623234736184c0d9a0b69e2ffbc124d", family: "votes", table: "user_votes"},
	/** `personalization.for_you_config`: one record (the table holds one row, `id` = true), mutable columns written by `put`. */
	forYouConfig: {id: "8a0e2bd56c834195b30db6a7eb966b50", family: "config", table: "personalization.for_you_config"},
	/** `entity_ranking_config`: one record (the table holds one row, `id` = true), mutable columns written by `put`. */
	entityRankingConfig: {id: "ee4e36fde6334be9b6d45085b5f6fb34", family: "config", table: "entity_ranking_config"},
	/** `personalization.interest_config`: one record (the table holds one row, `id` = true), mutable columns written by `put`. */
	interestConfig: {id: "e7ca641275a840bebb7ac20424a5deae", family: "config", table: "personalization.interest_config"},
	/** `personalization.interest_signal_weights`: one record per signal `kind` (text key), mutable columns written by `put`. */
	interestSignalWeight: {id: "1c35f5a4877f474a8d13835f8e05205d", family: "config", table: "personalization.interest_signal_weights"},
	/** `personalization.feed_experiments`: one record per experiment `id` (text key), mutable columns written by `put`. */
	feedExperiment: {id: "285dde3545964ab28ee580344b816704", family: "config", table: "personalization.feed_experiments"},
	/** `personalization.feed_experiment_members`: a record keyed (experiment id, user), one mutable column written by `put`. */
	feedExperimentMember: {id: "38b80c6e92bb4209838afdff3d7a0f01", family: "config", table: "personalization.feed_experiment_members"},
	/** `entity_type_weights`: one record per `type_id` (uuid key), mutable `weight` and `note` written by `put`. */
	entityTypeWeight: {id: "96a0897a18d240db9ce98df56649e1a6", family: "config", table: "entity_type_weights"},
	/** `entity_type_exclusions`: one record per excluded `type_id` (uuid key); a presence record with an optional `note`. */
	entityTypeExclusion: {id: "b3d4c18e3754433c95d3e31f628dca75", family: "config", table: "entity_type_exclusions"},
	/** `entity_feed_blocklist`: one record per blocked `entity_id` (uuid key); a presence record with an optional `reason`. */
	entityFeedBlock: {id: "1f351a8ff80c4a8c9c6c01b830d60a6f", family: "config", table: "entity_feed_blocklist"},
} as const satisfies Record<string, RecordKind>

/**
 * The kind whose singleton-per-family entity id is a system Space's founding nonce (see `systemSpaceNonce` in
 * systemSpaces.ts). Not a table; pinned the same way.
 */
export const SYSTEM_SPACE_NONCE_KIND = "0b3f6634fbe84dbd8c0ecae3fce960b3"

/**
 * Column property ids of keyed records, per kind: the property of each statement a record row carries. Minted once
 * as random UUID v4s (2026-11-05) and pinned like the kinds, append-only; `__tests__/recordKinds.test.ts` pins them.
 * A key column is written once per record (`add` + `set_active`, its id fixed by the target); a mutable column is a
 * `put` whose statement id includes the value, so a change closes the old value and a replay reopens the same id.
 */
export const RECORD_COLUMNS = {
	entityTypeWeight: {
		/** Key: text, `type_id` (lowercase dashed uuid). */
		typeId: "f8e4916a4af94f3784aaa2c3e55c8b7a",
		/** Mutable: decimal, `weight`. */
		weight: "36b085283e13476e9ccb657f5dade50a",
		/** Mutable: text, `note`; absent when the note is null. */
		note: "fed9b4ddefba4f238e36d89e765f3a3c",
	},
	entityTypeExclusion: {
		/** Key: text, `type_id` (lowercase dashed uuid). */
		typeId: "83cd54ae27b34a8688c3b3bc54d7d59c",
		/** Mutable: text, `note`; absent when the note is null. */
		note: "9c9d13fbf7e04b26bc2e841f227f3a10",
	},
	entityFeedBlock: {
		/** Key: text, `entity_id` (lowercase dashed uuid). */
		entityId: "e545438c24c54980a9aad2dedb9ff465",
		/** Mutable: text, `reason`; absent when the reason is null. */
		reason: "7c07f1706dbc44e3a654fd6e8c08f883",
	},
	feedExperimentMember: {
		/** Key: text, `experiment_id`. */
		experiment: "5478f85923d94274a98d6fbdb527e5f5",
		/** Key: text, `user_id` (lowercase dashed uuid). */
		user: "07d8f8c9c7884c039ae0ae4fc8f18f4c",
		/** Mutable: boolean, `interleaved`. */
		interleaved: "b1e33f48965b4c90af9b45ce368019e8",
	},
	feedExperiment: {
		/** Key: text, `id`. */
		id: "5302a53981a84da88c3949abe8165ec0",
		/** Mutable: boolean, `active`. */
		active: "d6a38e2b1f804353bceced2d49641d75",
		/** Mutable: text, `arm_a` (`best` or `for-you`). */
		armA: "5823d865cf6c480181ce0d3979b1a5ad",
		/** Mutable: text, `arm_b`. */
		armB: "62f4056881f34fcbbb26174932352e70",
		/** Mutable: decimal, `share`. */
		share: "20a413847f914c2ea382c24c69ffeaa9",
		/** Mutable: text, `salt`. */
		salt: "e3bc692a46a644179788ca4e9de3b6c5",
		/** Mutable: text, `note`; absent when the note is null. */
		note: "d9c6d4fef1584dd093f5314694ee1957",
	},
	interestConfig: {
		/** Key: number, `id` (always 1: the table's one row, `id` = true). */
		id: "31036382e13b4e11a4dc59c58a43546a",
		/** Mutable, one per column of the same name in snake case. Decimals except the three integers; `sweepLookbackSeconds` is `sweep_lookback` in seconds. */
		halfLifeDays: "1b50d435575f41aea81b5296b76eb480",
		spreadFraction: "1e771b65e07e4923bfa7c5b63620b349",
		spreadMaxNeighbours: "e47bea0f4de24e91bf4a2ebbbc7f2328",
		cooccurrenceMinSupport: "67060210b0214d4e907b53797e961f6f",
		cooccurrenceMaxTopicsPerEntity: "436f98f683fc4a38999b927ff08e49f0",
		sweepLookbackSeconds: "a4d8ed7e83ae486dbbae6d9e47db199b",
	},
	interestSignalWeight: {
		/** Key: text, `kind`. */
		kind: "63afb05ca56449a882b5506c760a82b9",
		/** Mutable: decimal, `weight`. */
		weight: "456ecbe7140a4d3bb399a9d28f4e9783",
		/** Mutable: boolean, `decays`. */
		decays: "ac260e6563f740f59fcdb97a64599c64",
		/** Mutable: text, `note`; absent when the note is null. */
		note: "525a71d896024df6ad030035680ad9ba",
	},
	entityRankingConfig: {
		/** Key: number, `id` (always 1: the table's one row, `id` = true). */
		id: "8aadf80a0b2546aea407d12a065494d1",
		/** Mutable, one per column of the same name in snake case; decimals except the five `*_target` / `*_days` integers. */
		tauSeconds: "5c83400bff444efca45f68eb07fa9b09",
		wilsonZ: "93a7b5019cd24ad99e52775c2241ee30",
		priorPositive: "76bd3657e4a94ea38786d51b4691ae80",
		priorNegative: "c4fd335a66204e149810f3e4e59d4381",
		qualityFloor: "dce4bf0f708948fb957c8bee3b2eac36",
		intrinsicCap: "f25971a4473f41ee9c07160abc6140f3",
		intrinsicPropertyTarget: "1969b6e8917c404e9c3aaaa51aedd99c",
		intrinsicRelationTarget: "98cff56a0dd84257afbf9443c3f808cb",
		participationWeight: "09edbee5458342e59ff36e54de66c3d9",
		participationCap: "b1944a17d2494980958f62485b2eb9bb",
		commentWeight: "5ac7a5ff10b4482cacda2a4c1ef2174f",
		commentCap: "3fd4bcd161cd40a8bc99eb9152e1a8be",
		topicInterestedWeight: "b5b8aacbbc554b48b0fcc659e95e6c85",
		topicDebateWeight: "307b57fd16a24d10877dd4a6d35a3409",
		topicCap: "87fc6d661db148779ea51c82f0cd09e9",
		watchWindowDays: "3c4a0b7b89504c9cb71bf52850478bb0",
		watchPriorImpressions: "5f05107bae2547a79055c7efb8297393",
		watchReferenceSeconds: "85744f77283c4019aefb2712facbd5ba",
		watchRatioWeight: "9a9c804f5f2048ef92d7c64dc2f3cae8",
		watchCompletedWeight: "a972ae8ed1524e6f909b5e2cdde6a3d3",
		watchPlaysWeight: "c96fc5fa9de641dab4de3809684e8bfb",
		watchClaimVotesWeight: "0f3f00fdfc5743c48b04cd5383526f63",
		watchCap: "70f00871fd7a40d88a2584b183ce9d40",
		watchFreshBoost: "548f6568cfa3408ca720715eabbd764e",
		watchFreshImpressions: "c45f3ac937aa479fb706c3af8bd9f360",
		trendingBoost: "026c5a72b15945c3b8c7b924eaf5c7c9",
		trendingTopicBoost: "269df22be38e45feb2ac2603a60100d6",
		trendingDayFactor: "3c05b3c405884a99a73004fdeb2d0c28",
		trendingDebateFactor: "7c5c04ba24df4ec2893305a3b351653b",
		trendingDebateWindowDays: "d39be3a05834407b86782dacdc025f79",
		trendingHorizonDays: "10bc0fc4779c43a9ba4ee03475f8ea26",
	},
	forYouConfig: {
		/** Key: number, `id` (always 1: the table's one row, `id` = true). */
		id: "054f9998f5204c85a2eeadc923fc614c",
		/** Mutable: decimal, `max_boost_days`. */
		maxBoostDays: "08242c8938ef411a900081d548a247b6",
		/** Mutable: decimal, `interest_half_saturation`. */
		interestHalfSaturation: "3aa038d4dd1c49778e4a11987a13d7d4",
		/** Mutable: decimal, `exploration_share`. */
		explorationShare: "9113b88fdcb24875a95175268f80aa57",
		/** Mutable: number, `max_topics`. */
		maxTopics: "53d2c96a35d4404e98c144aa1593ec2b",
	},
	rankingBlock: {
		/** Key: reference to the block entity, `cid("entity", id)`. */
		block: "30a0bcf61226497ea1a9fbdb05be369e",
		/** Key: reference to the Space entity, `cid("entity", space_id)`. */
		space: "723da53041a149e8bee03243676aec1a",
		/** Mutable: text, `name`. */
		name: "01c6cd80b4944f1eac79a2e069618dea",
		/** Mutable: text, `filter`. */
		filter: "34f4cb764c2d4c88a40ad7553254fcae",
		/** Mutable: instant (microseconds), `start_date`. */
		startDate: "a522112064024103937ca4a9a8347e0b",
		/** Mutable: instant (microseconds), `end_date`. */
		endDate: "a78b82b1e9384e409f367720aa0a9110",
		/** Mutable: reference, `restriction_id` (cid entity). */
		restriction: "02a37e407aed4aa09dd50080d8a86404",
		/** Mutable: reference, `ranking_type` (the rolling type, cid entity). */
		rankingType: "ff6fd916831643b2a87ea23f13254d11",
		/** Mutable: number, `submission_frequency`. */
		submissionFrequency: "ead65e7abfe64037aaef51050f4f2cc7",
	},
	ranking: {
		/** Key: reference to the ranking entity. */
		ranking: "5afd9abadca04814bb03eda81d951d22",
		/** Key: reference to the Space entity. */
		space: "c84de538157b4562be0352add400ee2f",
		/** Mutable: reference, `block_id` (from a RANK_BLOCK link, or the backfill). */
		block: "e702f42b27a94659a38f3ee7f8aef0b8",
		/** Mutable: text, `rank_type`. */
		rankType: "b943c785e30e4fa5a2f4c8886b4104de",
		/** Mutable: instant (microseconds), `submitted_at`. */
		submittedAt: "883ddfb02814461881f7ed2fdf0c6be5",
		/** Mutable: number, `updated_at_block`. */
		updatedAtBlock: "e2a7000398d848a7a5e541f63fd6c1a1",
		/** Mutable: number, `update_index`. */
		updateIndex: "529e8bb7b260483eb78cd68aa221a8a5",
		/** Mutable: text, `author_address`; only the backfill carries it. */
		author: "8acc92223b3643b39ceb4184c99d192a",
	},
	rankingItem: {
		/** Key: reference to the ranking entity. */
		ranking: "bc2d175676b04b5386b66ea4a3d6ffb3",
		/** Key: reference to the ranked entity. */
		entity: "3b05db08120843459c70cde796f14b68",
		/** Key: reference to the item's Space entity (`to_space`). */
		space: "565168847c344611964809b17abcf7a8",
		/** Mutable: text, `position` (the fractional index). */
		position: "d4ea2f12989646ce9aaec4f5aad414a2",
		/** Mutable: decimal, `weight`. */
		weight: "4ff16dd13cec48918186b04e30469eaf",
		/** Mutable: number, the block number of the edit that wrote the item; 0 for the backfill. A ranking's items are those at its maximum generation. */
		generation: "c37546c6d73b41f5aabb0fdd96fa16cb",
	},
	userVote: {
		/** Key: reference to the voter's entity, `cid("entity", user_id)`. */
		user: "4bd6d3068a204ff6a62bb084263045bc",
		/** Key: reference to the object; `cid("entity", id)` for an entity, `cid("statement", id)` for a relation. */
		object: "46e83cb03e0c46a4a874d7deeb5889dd",
		/** Key: number, `object_type` (0 entity, 1 relation). */
		objectType: "e2f5df668d07476ba3f1721c885bd53e",
		/** Key: reference to the Space-of-view's entity, `cid("entity", space_id)`. */
		space: "59217d4ba84c4fdcbdc3ff6fb3cb83fa",
		/** Key: number, `vote_kind` (0 curation, 1 stance, 2 veracity, 3 interested, or a newer raw value). */
		kind: "4f65ca0db5874a519d8068671a0222c5",
		/** Mutable: number, `vote_type` (0 up, 1 down, 2 removed). A removal keeps the row, as Postgres does. */
		direction: "8f3588d7a11c4e7eb86b61015670b491",
		/** Mutable: instant, `voted_at` (the event's block time, whole seconds). */
		votedAt: "6e088d8604924b05b09e3ebeceebf5db",
	},
	proposalVote: {
		/** Key: reference to the proposal's entity, `cid("entity", proposal_id)`. */
		proposal: "c020ebd69bbb4bec8c83db30765aa91b",
		/** Key: number, `proposal_version` (1 when the event carried 0, as kg-indexer stores it). */
		version: "da1d8b5c47ee499797e729ae90938086",
		/** Key: reference to the voting Space's entity, `cid("entity", voter_id)` (the voter is a Space). */
		voter: "e033f679eebb40aeaec8c66f9a1d6af9",
		/** Written once: reference to the entity of the Space that owns the proposal, `cid("entity", space_id)`. */
		space: "49a4ffb094684e10843bbc1e4c6a71f8",
		/** Mutable: number, `vote` (1 yes, 2 no, 3 abstain; none or an unknown option is 3, as kg-indexer stores it). */
		vote: "328dc69e2c964982a35fe4d3aa79e31a",
		/** Mutable: instant, `created_at` (the event's block time, whole seconds). */
		createdAt: "98603a29c209486b89a63ac207cf9e2f",
		/** Mutable: number, `created_at_block`. */
		createdAtBlock: "35243b2b183648059546576e381806c1",
	},
	spaceVotingSettings: {
		/** Key: reference to the Space's entity, `cid("entity", space_id)`. */
		space: "15ae9fc7333d4d72aeb372321e58db00",
		/** Mutable: number, `partial_percentage_support_threshold`. */
		partial: "a97262c9981f4663a20b4208cb2838d2",
		/** Mutable: number, `universal_percentage_support_threshold`. */
		universal: "4cf81d6cc92b4e0e91f2cb8139d736a2",
		/** Mutable: number, `flat_support_threshold`. */
		flat: "fcc01cfa9bee4781940a6be21fa67caa",
		/** Mutable: number, `quorum`. */
		quorum: "2cf27f7101af401d8e6de528d03b05d7",
		/** Mutable: number, `duration` (seconds). */
		duration: "4e371d0baf1d46d7a506491c192016ae",
		/** Mutable: boolean, `disable_fast_path_access_for_new_members`. */
		disableFastPath: "52570d2d1e95478ab9d31d811371a7dd",
		/** Mutable: number, `execution_grace_period`. */
		grace: "198ba2f858844b668a6126d5bc8a069e",
		/** Mutable: instant, `updated_at` (the event's block time, whole seconds). */
		updatedAt: "c23df2162f6b4b7b8bc41d740afc0f19",
		/** Mutable: number, `updated_at_block`. */
		updatedAtBlock: "8ba30f5e13da49699c351e1475c86f7b",
	},
	spaceTopic: {
		/** Key: reference to the Space's entity, `cid("entity", space_id)`. */
		space: "0249560173894904a354428215129068",
		/** Mutable: reference, `topic_id` (`cid("entity", topic_id)`); absent when the Space has no topic. */
		topic: "02b0149f64744e4d9ba1574e36e066ad",
	},
	spaceAddress: {
		/** Key: reference to the Space's entity, `cid("entity", space_id)`. */
		space: "9c7f59c747a544fd844cbffdeb5bbc48",
		/** Mutable: text, `spaces.address` as kg-indexer stores it (EIP-55 from a create, lowercase from an update). */
		address: "a584f4100b8d47428afd4242371c44c8",
	},
	proposalAction: {
		/** Key: reference to the proposal's entity, `cid("entity", proposal_id)`. */
		proposal: "235abc0092a248d69e2dbb5c71fd0209",
		/** Key: number, the block of the event that wrote this action's version (`proposal_versions.version_created_at_block`). */
		block: "a93c92d0a2f442e9852057064567e012",
		/** Key: number, `index`, the action's position in its version's action list. */
		index: "81b028b2bb7f4f9e85e5689a2f1826af",
		/** Mutable: text, `action_type` (`AddMember` ... `Unknown`, exactly as Postgres's enum prints). */
		actionType: "002e8db762e0401386b53478b329013e",
		/** Mutable: reference to `cid("entity", target_id)`, when the action has a target. */
		target: "afc49d0ddf9c4affaae9ff963338d7c4",
		/** Mutable: text, `content_uri` (Publish). */
		contentUri: "a267fc081a4c457abfda85b8fc024d92",
		/** Mutable: text, `metadata` as lowercase hex (Publish, when non-empty). */
		metadata: "5ee665ce733b46ffa8d6697c1b2cedcb",
		/** Mutable: text, `content_id` as lowercase hex (Flag, Unflag). */
		contentId: "268e38bcf4d247138d7dbd154873391c",
		/** Mutable: number, `quorum` (UpdateVotingSettings). */
		quorum: "83b46fb278414103866bcba416f7c2d2",
		/** Mutable: number, `duration` (UpdateVotingSettings). */
		duration: "bbf27d7c47634001b35d46ace45371ff",
		/** Mutable: number, `partial_percentage_support_threshold` (UpdateVotingSettings; legacy `slow_threshold` equals it). */
		partial: "c13d280d658247d0859a3c0ce0367da4",
		/** Mutable: number, `universal_percentage_support_threshold` (UpdateVotingSettings). */
		universal: "603b264f392c4aa5be3d9819bc04b106",
		/** Mutable: number, `flat_support_threshold` (UpdateVotingSettings; legacy `fast_threshold` equals it). */
		flat: "d0d34304079149e78157e70d6b3722d4",
		/** Mutable: boolean, `disable_fast_path_access_for_new_members` (UpdateVotingSettings). */
		disableFastPath: "4a13aa7d7f55476fb307b2d19b14dfe7",
		/** Mutable: number, `execution_grace_period` (UpdateVotingSettings). */
		grace: "401c3bec5a414703a5b6b51ce8e18e13",
	},
	proposalStatus: {
		/** Key: reference to the proposal's entity, `cid("entity", proposal_id)`. */
		proposal: "4f742a5f821a469c9c0be0b352ae5b67",
		/** Mutable: instant, `executed_at` (PROPOSAL_EXECUTED block time, whole seconds). */
		executedAt: "ed3855a3c41b4f749651bf049dabac0b",
		/** Mutable: instant, `unexecutable_at` (operator backfill; no event). */
		unexecutableAt: "72179d56e3dc478d9c9ab2159e4b40ef",
	},
	proposalEscalation: {
		/** Key: reference to the proposal's entity, `cid("entity", proposal_id)`. */
		proposal: "49c19599b0694221841396f0db5f9995",
		/** Key: number, the block of the PROPOSAL_SETTINGS_UPDATED event. */
		block: "89cbe515c863439dbf2a7686c676c90a",
		/** Mutable: text, `voting_mode` the escalation set (`Fast` or `Slow`). */
		votingMode: "e3f7ba26068849658179f6d443b0a064",
		/** Mutable: number, `start_time`. */
		startTime: "0c2fcb7931a34133a8a4258ae111f20e",
		/** Mutable: number, `end_time`. */
		endTime: "31d689b4544b4fcd91af181db291a4d8",
		/** Mutable: number, `quorum`. */
		quorum: "43a88c623ad243cebadbff37b0e6d567",
		/** Mutable: number, `partial_percentage_support_threshold`. */
		partial: "4e2cf0e3c88d4b998b9a4932469f702e",
		/** Mutable: number, `universal_percentage_support_threshold`. */
		universal: "d4124abe7be548548d39f67383669975",
		/** Mutable: number, `flat_support_threshold`. */
		flat: "473d72f8ac84495284b3623afef491a0",
		/** Mutable: number, `execute_by`, when the escalated settings have a deadline (absent: none, as the UPDATE writes NULL). */
		executeBy: "7ef74c02759c4e658a3a73e18f8f0733",
	},
	subspace: {
		/** Edge property for a `verified` subspace row: parent Space entity -> child Space entity. */
		verified: "2c527e632f3645cdbbc2c2a9806dfc05",
		/** Edge property for a `related` subspace row. */
		related: "ba99f0b8bf4646ed9cdb404da918e5e1",
	},
	subspaceTopic: {
		/** Edge property for a `subspace_topics` row: Space entity -> topic entity. */
		topic: "90ee068ad2d44a8b95cf939d2d1ceabb",
	},
	proposalVersion: {
		/** Key: reference to the proposal's entity, `cid("entity", proposal_id)`. */
		proposal: "c42be9e340d14566b2b9f084004c748b",
		/** Key: number, `version_created_at_block`, the block of the event that wrote the version (its idempotency key in Postgres). */
		block: "bccb84d7017849b0babfa7444789068f",
		/** Mutable: text, `voting_mode` (`Fast` or `Slow`, as Postgres's enum prints). */
		votingMode: "1ec5fb72263443668b4880d09670cb56",
		/** Mutable: number, `start_time`. */
		startTime: "5a806f60759e4dd08a39c771d7586f53",
		/** Mutable: number, `end_time`. */
		endTime: "72d87497d7ed49db81e31675a31c5da7",
		/** Mutable: number, `quorum`. */
		quorum: "54000906476b41b7a0ca655f043b1a8d",
		/** Mutable: number, `partial_percentage_support_threshold` (legacy `threshold` equals it for Slow). */
		partial: "e4e76529227940f08bd1ca1ab884fe31",
		/** Mutable: number, `universal_percentage_support_threshold`. */
		universal: "eecb13aaa70c44509070df7550436103",
		/** Mutable: number, `flat_support_threshold` (legacy `threshold` equals it for Fast). */
		flat: "2e641ff402734c6b80201641f3969fe2",
		/** Mutable: number, `execute_by`, when the version has a deadline. */
		executeBy: "ca75ecfb5ab24196998de1b3bb31eb6f",
		/** Mutable: text, `name`, when the version has actions. */
		name: "a76bbfa24cfa43708b89326ae5f72e18",
		/** Mutable: number, `version_created_at` (unix seconds, 0 when the event has no metadata). */
		createdAt: "6c74f4f248d249368e1002b4c393a59e",
	},
} as const satisfies Record<string, Record<string, string>>
