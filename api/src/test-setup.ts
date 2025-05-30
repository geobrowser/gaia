import { afterEach, beforeEach, vi } from 'vitest'

// Make vi globally available
// @ts-expect-error globalThis is not typed
globalThis.vi = vi

// Setup global test utilities
beforeEach(() => {
  vi.clearAllMocks()
})

afterEach(() => {
  vi.resetAllMocks()
})
