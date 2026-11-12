import {describe, expect, it} from "vitest"
import {type RankingInputs, type RankingSource, rankingMemberBallots} from "../trending"

/**
 * GEO-3591 (0125): a Rank id registered from two spaces is two ballots, and `trending.ranking_member_ballots` joins
 * items on the ballot's own (Rank id, space). The golden has one space per Rank id, so it can't tell the two joins apart.
 */
describe("rankingMemberBallots", () => {
	const source: RankingSource = {
		name: "s",
		space_id: "space",
		block_id: "block",
		weight: 1,
		window_days: null,
		slots: 1,
		day_factor: 1,
	}
	const inputs = (items: RankingInputs["items"]): RankingInputs => ({
		scores: [],
		ballots: ["m1", "m2"].map((m) => ({
			id: "rank",
			block_id: "block",
			space_id: m,
			updated_at_block: 10,
			update_index: 0,
		})),
		items,
		members: [
			{member_space_id: "m1", space_id: "space"},
			{member_space_id: "m2", space_id: "space"},
		],
		votes: [],
		rankable: new Map(),
	})

	it("keeps each space's items on that space's ballot", () => {
		const got = rankingMemberBallots(
			[source],
			inputs([
				{ranking_id: "rank", entity_id: "a", rank_space_id: "m1"},
				{ranking_id: "rank", entity_id: "c", rank_space_id: "m2"},
			]),
		)
		expect(got.map(([, , member, entity]) => `${member}:${entity}`).sort()).toEqual(["m1:a", "m2:c"])
	})

	it("matches an item without a rank space by ranking id alone", () => {
		const got = rankingMemberBallots([source], inputs([{ranking_id: "rank", entity_id: "a"}]))
		expect(got.map(([, , member, entity]) => `${member}:${entity}`).sort()).toEqual(["m1:a", "m2:a"])
	})
})
