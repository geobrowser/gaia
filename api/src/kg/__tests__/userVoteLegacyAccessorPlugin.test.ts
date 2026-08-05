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
const U = "00000000-cafe-4000-8000-000000000001"
const O = "00000000-cafe-4000-8000-000000000002"
const S = "00000000-cafe-4000-8000-000000000003"
const undash = (s: string) => s.replace(/-/g, "")

// Guards the compat shim restoring the unique-key accessor that migration 0071
// renamed. The shipped web client queries the old name, so if these fail the
// live vote controls lose the user's own vote state.
describe("UserVoteLegacyAccessorPlugin", () => {
	let pool: Pool
	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await pool.query(`DELETE FROM user_votes WHERE user_id=$1`, [U])
		// curation = DOWN(1), stance = UP(0), veracity = UP(0)
		await pool.query(
			`INSERT INTO user_votes (user_id,object_id,object_type,space_id,vote_type,vote_kind,voted_at) VALUES
			 ($1,$2,0,$3,1,0,now()), ($1,$2,0,$3,0,1,now()), ($1,$2,0,$3,0,2,now())`,
			[U, O, S],
		)
	})
	afterAll(async () => {
		await pool.query(`DELETE FROM user_votes WHERE user_id=$1`, [U])
		await pool.end()
	})

	it("the legacy field exists again", async () => {
		const r = await q(`{ __type(name:"Query"){ fields { name } } }`)
		const names = (r.data.__type as unknown as {fields: Array<{name: string}>}).fields.map((f) => f.name)
		expect(names).toContain("userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId")
		expect(names).toContain("userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceIdAndVoteKind")
	})

	it("resolves the CURATION row, not stance or veracity", async () => {
		const r = await q(
			`query UserEntityVote($userId: UUID!, $objectId: UUID!, $objectType: Int!, $spaceId: UUID!) {
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId(userId:$userId, objectId:$objectId, objectType:$objectType, spaceId:$spaceId) { voteType }
			}`,
			{userId: undash(U), objectId: undash(O), objectType: 0, spaceId: undash(S)},
		)
		expect(r.errors).toBeUndefined()
		// curation row is voteType 1; stance/veracity are 0. Must see 1.
		const row = r.data.userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId as unknown as {voteType: number}
		expect(row.voteType).toBe(1)
	})

	it("returns null when the user has no curation row", async () => {
		await pool.query(`DELETE FROM user_votes WHERE user_id=$1 AND vote_kind=0`, [U])
		const r = await q(
			`query UserEntityVote($userId: UUID!, $objectId: UUID!, $objectType: Int!, $spaceId: UUID!) {
				userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId(userId:$userId, objectId:$objectId, objectType:$objectType, spaceId:$spaceId) { voteType }
			}`,
			{userId: undash(U), objectId: undash(O), objectType: 0, spaceId: undash(S)},
		)
		expect(r.errors).toBeUndefined()
		// stance + veracity rows still exist, but must not be returned
		expect(r.data.userVoteByUserIdAndObjectIdAndObjectTypeAndSpaceId).toBeNull()
	})
})
