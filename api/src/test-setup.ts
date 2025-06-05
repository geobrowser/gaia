import {afterEach, beforeEach, vi} from "vitest"

// Make vi globally available
globalThis.vi = vi

// Setup global test utilities
beforeEach(() => {
	vi.clearAllMocks()
})

afterEach(() => {
	vi.resetAllMocks()
})
