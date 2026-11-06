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
	/** `proposal_actions`: a record keyed (proposal, action index). */
	proposalAction: {id: "22335b74e17a4c9e867d4ac2a32c3a4a", family: "governance", table: "proposal_actions"},
	/** `space_voting_settings`: a record keyed (space), mutable columns written by `put`. */
	spaceVotingSettings: {id: "7767f888a9a240b18275a7832363f4d9", family: "governance", table: "space_voting_settings"},
	/** `proposal_votes`: a record keyed (proposal, proposal version, voter), its primary key. */
	proposalVote: {id: "ca89a877762c4883a3640b5455a8f0e2", family: "votes", table: "proposal_votes"},
	/** `user_votes`: a record keyed (user, object, object type, space, kind). */
	userVote: {id: "1623234736184c0d9a0b69e2ffbc124d", family: "votes", table: "user_votes"},
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
} as const satisfies Record<string, Record<string, string>>
