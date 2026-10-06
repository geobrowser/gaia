import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

type GqlResponse = {
	errors?: Array<{message: string}>
	data: Record<string, unknown>
}

const q = async (query: string, variables?: Record<string, unknown>): Promise<GqlResponse> => {
	const r = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return r.json() as Promise<GqlResponse>
}

const VIEWER = "00000000-3158-4000-8000-000000000001"
const OTHER = "00000000-3158-4000-8000-000000000002"
const TOPIC = "00000000-3158-4000-8000-000000000003"
const SPACE_A = "00000000-3158-4000-8000-000000000004"
const SPACE_B = "00000000-3158-4000-8000-000000000005"
const undash = (s: string) => s.replace(/-/g, "")

// GEO-3158. Interested is vote_kind 3 and needs no schema of its own: these pin that the
// generated API serves it the way it serves stance, which is what the web client reads.
describe("Interested (vote_kind 3) over GraphQL", () => {
	let pool: Pool
	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM user_votes WHERE object_id=$1`, [TOPIC])
		await pool.query(`DELETE FROM votes_count WHERE object_id=$1`, [TOPIC])
		// The viewer holds Interested and a curation upvote on the topic; another user holds a
		// cleared Interested (vote_type 2).
		await pool.query(
			`INSERT INTO user_votes (user_id,object_id,object_type,space_id,vote_type,vote_kind,voted_at) VALUES
			 ($1,$3,0,$4,0,3,now()), ($1,$3,0,$4,0,0,now()), ($2,$3,0,$4,2,3,now())`,
			[VIEWER, OTHER, TOPIC, SPACE_A],
		)
		// Interested tallies in two spaces, and a curation tally that must not mix in.
		await pool.query(
			`INSERT INTO votes_count (object_id,object_type,space_id,vote_kind,positive,negative) VALUES
			 ($1,0,$2,3,4,0), ($1,0,$3,3,2,0), ($1,0,$2,0,9,1)`,
			[TOPIC, SPACE_A, SPACE_B],
		)
	})
	afterAll(async () => {
		await pool.query(`DELETE FROM user_votes WHERE object_id=$1`, [TOPIC])
		await pool.query(`DELETE FROM votes_count WHERE object_id=$1`, [TOPIC])
		await pool.end()
	})

	it("serves an entity's Interested count per space, apart from its curation tally", async () => {
		const r = await q(
			`query($objectId: UUID!) {
				votesCountsConnection(condition: {objectId: $objectId, objectType: 0, voteKind: 3}) {
					nodes { spaceId positive negative voteKind }
				}
			}`,
			{objectId: undash(TOPIC)},
		)
		expect(r.errors).toBeUndefined()
		const nodes = (
			r.data.votesCountsConnection as {nodes: Array<{positive: string; negative: string; voteKind: number}>}
		).nodes
		expect(nodes).toHaveLength(2)
		expect(nodes.every((n) => n.voteKind === 3 && Number(n.negative) === 0)).toBe(true)
		expect(nodes.reduce((sum, n) => sum + Number(n.positive), 0)).toBe(6)
	})

	it("serves one space's Interested count through the per-kind accessor", async () => {
		const r = await q(
			`query($objectId: UUID!, $spaceId: UUID!) {
				votesCountByObjectIdAndObjectTypeAndSpaceIdAndVoteKind(objectId: $objectId, objectType: 0, spaceId: $spaceId, voteKind: 3) {
					positive
				}
			}`,
			{objectId: undash(TOPIC), spaceId: undash(SPACE_A)},
		)
		expect(r.errors).toBeUndefined()
		expect(
			Number((r.data.votesCountByObjectIdAndObjectTypeAndSpaceIdAndVoteKind as {positive: string}).positive),
		).toBe(4)
	})

	it("serves the viewer's own Interested state, and a cleared one reads as cleared", async () => {
		const query = `query($userId: UUID!, $objectId: UUID!, $spaceId: UUID!) {
			userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind(userId: $userId, objectId: $objectId, objectType: 0, spaceId: $spaceId, voteKind: 3) {
				voteType
			}
		}`
		const mine = await q(query, {userId: undash(VIEWER), objectId: undash(TOPIC), spaceId: undash(SPACE_A)})
		expect(mine.errors).toBeUndefined()
		expect(mine.data.userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind).toEqual({voteType: 0})

		const cleared = await q(query, {userId: undash(OTHER), objectId: undash(TOPIC), spaceId: undash(SPACE_A)})
		expect(cleared.data.userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind).toEqual({voteType: 2})
	})

	it("lists every topic a user is Interested in, which is how the web client reads follows", async () => {
		const r = await q(
			`query($userId: UUID!) {
				userVotesConnection(condition: {userId: $userId, voteKind: 3, voteType: 0, objectType: 0}, first: 100) {
					nodes { objectId spaceId }
				}
			}`,
			{userId: undash(VIEWER)},
		)
		expect(r.errors).toBeUndefined()
		const nodes = (r.data.userVotesConnection as {nodes: Array<{objectId: string}>}).nodes
		expect(nodes.map((n) => undash(n.objectId))).toEqual([undash(TOPIC)])
	})

	it("leaves the legacy curation accessor on the curation row", async () => {
		const r = await q(
			`query($userId: UUID!, $objectId: UUID!, $spaceId: UUID!) {
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId(userId: $userId, objectId: $objectId, objectType: 0, spaceId: $spaceId) { voteKind }
			}`,
			{userId: undash(VIEWER), objectId: undash(TOPIC), spaceId: undash(SPACE_A)},
		)
		expect(r.errors).toBeUndefined()
		expect(r.data.userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId).toEqual({voteKind: 0})
	})
})
