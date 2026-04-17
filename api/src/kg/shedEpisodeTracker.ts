import type {SaturationSnapshot} from "../services/dbSaturation"

/**
 * Tracks which saturation episode has already produced an onset log. A new
 * episode is identified by its `activeSince` ISO timestamp, which the
 * saturation FSM sets when it first flips `isSaturated: true` and clears on
 * release. The tracker answers two questions on each shed check:
 *
 * - `shouldLogOnset(snapshot)` — is this the first shed of a new episode?
 *   Returns true exactly once per episode (the first call with a not-yet-seen
 *   `activeSince`). Also clears the marker when `isSaturated` drops, so the
 *   next episode logs its own onset.
 *
 * Pure state container — no logging or metrics side effects — so it can be
 * unit-tested without mocks.
 */
export type ShedEpisodeTracker = {
	shouldLogOnset(snapshot: SaturationSnapshot): boolean
}

export function createShedEpisodeTracker(): ShedEpisodeTracker {
	let lastLoggedActiveSince: string | null = null

	return {
		shouldLogOnset(snapshot) {
			if (!snapshot.isSaturated) {
				// Release edge: clear marker so the next episode logs again.
				lastLoggedActiveSince = null
				return false
			}

			if (snapshot.activeSince === null) {
				// Defensive: isSaturated should imply activeSince is set.
				return false
			}

			if (snapshot.activeSince === lastLoggedActiveSince) {
				return false
			}

			lastLoggedActiveSince = snapshot.activeSince
			return true
		},
	}
}
