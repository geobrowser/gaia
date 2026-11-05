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
	/** `proposal_votes`: a record keyed (proposal, voter). */
	proposalVote: {id: "ca89a877762c4883a3640b5455a8f0e2", family: "votes", table: "proposal_votes"},
	/** `user_votes`: a record keyed (user, object, object type, space, kind). */
	userVote: {id: "1623234736184c0d9a0b69e2ffbc124d", family: "votes", table: "user_votes"},
} as const satisfies Record<string, RecordKind>

/**
 * The kind whose singleton-per-family entity id is a system Space's founding nonce (see `systemSpaceNonce` in
 * systemSpaces.ts). Not a table; pinned the same way.
 */
export const SYSTEM_SPACE_NONCE_KIND = "0b3f6634fbe84dbd8c0ecae3fce960b3"
