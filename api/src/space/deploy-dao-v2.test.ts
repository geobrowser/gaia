import {describe, expect, it} from "vitest"
import {validateVotingSettings, RATIO_BASE, MINIMUM_VOTING_DURATION} from "./deploy-dao-v2"

describe("validateVotingSettings", () => {
	const validSettings = {
		slowPathPercentageThreshold: BigInt(5000), // 50%
		fastPathFlatThreshold: BigInt(1),
		quorum: BigInt(1),
		duration: MINIMUM_VOTING_DURATION,
	}

	describe("slowPathPercentageThreshold", () => {
		it("should reject threshold greater than RATIO_BASE (10000)", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: RATIO_BASE + BigInt(1)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toContain("slowPathPercentageThreshold")
			expect(error).toContain("10000")
		})

		it("should accept threshold equal to RATIO_BASE", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: RATIO_BASE}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})

		it("should accept threshold of 0", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: BigInt(0)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("fastPathFlatThreshold", () => {
		it("should reject threshold greater than total editors", () => {
			const settings = {...validSettings, fastPathFlatThreshold: BigInt(5)}
			const error = validateVotingSettings(settings, 3)
			expect(error).toContain("fastPathFlatThreshold")
			expect(error).toContain("3")
		})

		it("should accept threshold equal to total editors", () => {
			const settings = {...validSettings, fastPathFlatThreshold: BigInt(3)}
			const error = validateVotingSettings(settings, 3)
			expect(error).toBeNull()
		})

		it("should accept threshold of 0", () => {
			const settings = {...validSettings, fastPathFlatThreshold: BigInt(0)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("quorum", () => {
		it("should reject quorum greater than total editors", () => {
			const settings = {...validSettings, quorum: BigInt(10)}
			const error = validateVotingSettings(settings, 5)
			expect(error).toContain("quorum")
			expect(error).toContain("5")
		})

		it("should accept quorum equal to total editors", () => {
			const settings = {...validSettings, quorum: BigInt(5)}
			const error = validateVotingSettings(settings, 5)
			expect(error).toBeNull()
		})

		it("should accept quorum of 0", () => {
			const settings = {...validSettings, quorum: BigInt(0)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("duration", () => {
		it("should reject duration less than MINIMUM_VOTING_DURATION (2 days)", () => {
			const settings = {...validSettings, duration: MINIMUM_VOTING_DURATION - BigInt(1)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toContain("duration")
			expect(error).toContain("172800")
		})

		it("should accept duration equal to MINIMUM_VOTING_DURATION", () => {
			const settings = {...validSettings, duration: MINIMUM_VOTING_DURATION}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})

		it("should accept duration greater than MINIMUM_VOTING_DURATION", () => {
			const settings = {...validSettings, duration: MINIMUM_VOTING_DURATION + BigInt(86400)}
			const error = validateVotingSettings(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("valid settings", () => {
		it("should return null for valid settings with 1 editor", () => {
			const error = validateVotingSettings(validSettings, 1)
			expect(error).toBeNull()
		})

		it("should return null for valid settings with multiple editors", () => {
			const settings = {
				slowPathPercentageThreshold: BigInt(5000),
				fastPathFlatThreshold: BigInt(3),
				quorum: BigInt(2),
				duration: MINIMUM_VOTING_DURATION,
			}
			const error = validateVotingSettings(settings, 5)
			expect(error).toBeNull()
		})
	})
})
