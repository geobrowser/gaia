import {describe, expect, it} from "vitest"
import {
	validateVotingSettingsInput,
	percentageToRatio,
	daysToSeconds,
	toContractVotingSettings,
	RATIO_BASE,
	MINIMUM_VOTING_DURATION,
	MINIMUM_VOTING_DURATION_DAYS,
} from "./deploy-dao-v2"

describe("percentageToRatio", () => {
	it("should convert 100% to RATIO_BASE", () => {
		expect(percentageToRatio(100)).toBe(RATIO_BASE)
	})

	it("should convert 50% to half of RATIO_BASE", () => {
		expect(percentageToRatio(50)).toBe(RATIO_BASE / BigInt(2))
	})

	it("should convert 0% to 0", () => {
		expect(percentageToRatio(0)).toBe(BigInt(0))
	})

	it("should handle decimal percentages", () => {
		expect(percentageToRatio(33.33)).toBe(BigInt(3333000))
	})
})

describe("daysToSeconds", () => {
	it("should convert 1 day to 86400 seconds", () => {
		expect(daysToSeconds(1)).toBe(BigInt(86400))
	})

	it("should convert 2 days to 172800 seconds", () => {
		expect(daysToSeconds(2)).toBe(MINIMUM_VOTING_DURATION)
	})

	it("should handle fractional days", () => {
		expect(daysToSeconds(0.5)).toBe(BigInt(43200))
	})
})

describe("toContractVotingSettings", () => {
	it("should convert user-friendly settings to contract format", () => {
		const input = {
			slowPathPercentageThreshold: 50,
			fastPathFlatThreshold: 3,
			quorum: 2,
			durationInDays: 7,
		}

		const result = toContractVotingSettings(input)

		expect(result.slowPathPercentageThreshold).toBe(BigInt(5e6))
		expect(result.fastPathFlatThreshold).toBe(BigInt(3))
		expect(result.quorum).toBe(BigInt(2))
		expect(result.duration).toBe(BigInt(7 * 24 * 60 * 60))
	})
})

describe("validateVotingSettingsInput", () => {
	const validSettings = {
		slowPathPercentageThreshold: 50,
		fastPathFlatThreshold: 1,
		quorum: 1,
		durationInDays: MINIMUM_VOTING_DURATION_DAYS,
	}

	describe("slowPathPercentageThreshold", () => {
		it("should reject percentage greater than 100", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: 101}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toContain("slowPathPercentageThreshold")
			expect(error).toContain("0 and 100")
		})

		it("should reject negative percentage", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: -1}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toContain("slowPathPercentageThreshold")
		})

		it("should accept percentage equal to 100", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: 100}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})

		it("should accept percentage of 0", () => {
			const settings = {...validSettings, slowPathPercentageThreshold: 0}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("fastPathFlatThreshold", () => {
		it("should reject threshold greater than total editors", () => {
			const settings = {...validSettings, fastPathFlatThreshold: 5}
			const error = validateVotingSettingsInput(settings, 3)
			expect(error).toContain("fastPathFlatThreshold")
			expect(error).toContain("3")
		})

		it("should reject negative threshold", () => {
			const settings = {...validSettings, fastPathFlatThreshold: -1}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toContain("fastPathFlatThreshold")
		})

		it("should accept threshold equal to total editors", () => {
			const settings = {...validSettings, fastPathFlatThreshold: 3}
			const error = validateVotingSettingsInput(settings, 3)
			expect(error).toBeNull()
		})

		it("should accept threshold of 0", () => {
			const settings = {...validSettings, fastPathFlatThreshold: 0}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("quorum", () => {
		it("should reject quorum greater than total editors", () => {
			const settings = {...validSettings, quorum: 10}
			const error = validateVotingSettingsInput(settings, 5)
			expect(error).toContain("quorum")
			expect(error).toContain("5")
		})

		it("should reject negative quorum", () => {
			const settings = {...validSettings, quorum: -1}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toContain("quorum")
		})

		it("should accept quorum equal to total editors", () => {
			const settings = {...validSettings, quorum: 5}
			const error = validateVotingSettingsInput(settings, 5)
			expect(error).toBeNull()
		})

		it("should accept quorum of 0", () => {
			const settings = {...validSettings, quorum: 0}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("durationInDays", () => {
		it("should reject duration less than minimum (2 days)", () => {
			const settings = {...validSettings, durationInDays: 1}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toContain("durationInDays")
			expect(error).toContain("2")
		})

		it("should accept duration equal to minimum", () => {
			const settings = {...validSettings, durationInDays: MINIMUM_VOTING_DURATION_DAYS}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})

		it("should accept duration greater than minimum", () => {
			const settings = {...validSettings, durationInDays: 7}
			const error = validateVotingSettingsInput(settings, 1)
			expect(error).toBeNull()
		})
	})

	describe("valid settings", () => {
		it("should return null for valid settings with 1 editor", () => {
			const error = validateVotingSettingsInput(validSettings, 1)
			expect(error).toBeNull()
		})

		it("should return null for valid settings with multiple editors", () => {
			const settings = {
				slowPathPercentageThreshold: 50,
				fastPathFlatThreshold: 3,
				quorum: 2,
				durationInDays: 7,
			}
			const error = validateVotingSettingsInput(settings, 5)
			expect(error).toBeNull()
		})
	})
})
