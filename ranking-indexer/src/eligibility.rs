//! Eligibility resolution for ranking aggregation (design §7).
//!
//! A submission counts toward a block only if it passes both a *restriction*
//! check and a *window* check.
//!
//! Restriction (per the team decision):
//!   - DAO-space block: the submitter must be a member or editor of the block's
//!     space.
//!   - Personal-space block: "All of Geo" for now — admit every submitter. (This
//!     will narrow to verified users once verification ships.)
//!
//! Window: for a static block, the submission timestamp must fall within the
//! block's optional `[start, end]` bounds; an absent bound imposes no limit.
//! For a Rolling block (GEO-2328), instead a submission stays eligible only
//! while `now - submitted_at < submission_frequency` — it ages out and
//! requires resubmission rather than being bounded by a fixed window.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{Ranking, RankingBlock};

/// The kind of space a block lives in, which sets its default restriction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceKind {
    /// Governed space — restrict to members/editors.
    Dao,
    /// Personal space — "All of Geo" for now.
    Personal,
}

/// Does the block's restriction admit a submission from `submitter_space`?
pub fn restriction_admits(
    space_kind: SpaceKind,
    submitter_space: Uuid,
    eligible_member_spaces: &HashSet<Uuid>,
) -> bool {
    match space_kind {
        // "All of Geo" for now; narrows to verified users post-verification.
        SpaceKind::Personal => true,
        SpaceKind::Dao => eligible_member_spaces.contains(&submitter_space),
    }
}

/// Does the submission fall within the block's optional window?
///
/// An absent bound imposes no limit. If the submission time is unknown the
/// window cannot reject it — populating `submitted_at` in `detect()` is a
/// follow-up, so we admit rather than silently drop.
pub fn window_admits(
    submitted_at: Option<DateTime<Utc>>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> bool {
    let after_start = match (start, submitted_at) {
        (Some(s), Some(t)) => t >= s,
        _ => true,
    };
    let before_end = match (end, submitted_at) {
        (Some(e), Some(t)) => t <= e,
        _ => true,
    };
    after_start && before_end
}

/// Does a Rolling-type block's submission remain eligible?
///
/// `submission_frequency` is a *half-life*, not a cliff: a ballot's contribution
/// decays continuously (see `scoring::recency_weight`) and this check only drops
/// it once decay has taken it below the resolution of the published projection,
/// at `ROLLING_MAX_HALF_LIVES` half-lives.
///
/// It used to expire at exactly one `submission_frequency`, which made a block
/// publish nothing whenever nobody had submitted inside the trailing window —
/// the common case, since the window equalled the cadence users were told to
/// resubmit on. That rendered a "Trending" table as empty rather than stale, and
/// it also drove the client to blank the author's ballot on roll-off.
///
/// `now` is a parameter (never read via an inline `Utc::now()` call here) so
/// callers — tests and the periodic sweep alike — control it deterministically.
///
/// An absent frequency or unknown submission time can't be evaluated, so (like
/// `window_admits`) we admit rather than silently drop.
pub fn rolling_admits(
    submitted_at: Option<DateTime<Utc>>,
    frequency_hours: Option<i32>,
    now: DateTime<Utc>,
) -> bool {
    let (Some(submitted_at), Some(frequency_hours)) = (submitted_at, frequency_hours) else {
        return true;
    };
    if frequency_hours <= 0 {
        return true;
    }
    let cutoff_hours = f64::from(frequency_hours) * crate::scoring::ROLLING_MAX_HALF_LIVES;
    let age_hours = (now - submitted_at).num_milliseconds() as f64 / 3_600_000.0;
    age_hours < cutoff_hours
}

/// Filter deduped submissions to those eligible for `block`.
///
/// `eligible_member_spaces` is the set of member/editor space ids of the
/// block's space (ignored for personal-space blocks). `now` is the instant
/// used to evaluate a Rolling block's per-submission expiry.
pub fn filter_eligible(
    block: &RankingBlock,
    space_kind: SpaceKind,
    eligible_member_spaces: &HashSet<Uuid>,
    submissions: Vec<Ranking>,
    now: DateTime<Utc>,
) -> Vec<Ranking> {
    let is_rolling = block.ranking_type.is_some();
    submissions
        .into_iter()
        .filter(|s| {
            restriction_admits(space_kind, s.space_id, eligible_member_spaces)
                && if is_rolling {
                    rolling_admits(s.submitted_at, block.submission_frequency, now)
                } else {
                    window_admits(s.submitted_at, block.start_date, block.end_date)
                }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> RankingBlock {
        RankingBlock {
            id: Uuid::from_u128(1),
            space_id: Uuid::from_u128(900),
            name: None,
            filter: None,
            start_date: start,
            end_date: end,
            restriction_id: None,
            ranking_type: None,
            submission_frequency: None,
        }
    }

    fn rolling_block(frequency_hours: Option<i32>) -> RankingBlock {
        RankingBlock {
            ranking_type: Some(Uuid::from_u128(2)), // any value marks it Rolling
            submission_frequency: frequency_hours,
            ..block(None, None)
        }
    }

    fn submission(space: u128, at: Option<DateTime<Utc>>) -> Ranking {
        Ranking {
            id: Uuid::from_u128(space + 1000),
            block_id: Some(Uuid::from_u128(1)),
            space_id: Uuid::from_u128(space),
            author_address: None,
            rank_type: None,
            submitted_at: at,
            updated_at_block: 0,
            update_index: 0,
        }
    }

    #[test]
    fn personal_block_admits_any_submitter() {
        let members = HashSet::new(); // none
        assert!(restriction_admits(
            SpaceKind::Personal,
            Uuid::from_u128(42),
            &members
        ));
    }

    #[test]
    fn dao_block_admits_only_members_or_editors() {
        let mut members = HashSet::new();
        members.insert(Uuid::from_u128(42));
        assert!(restriction_admits(
            SpaceKind::Dao,
            Uuid::from_u128(42),
            &members
        ));
        assert!(!restriction_admits(
            SpaceKind::Dao,
            Uuid::from_u128(99),
            &members
        ));
    }

    #[test]
    fn window_bounds() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0);
        // within
        assert!(window_admits(t(50), t(0), t(100)));
        // before start / after end
        assert!(!window_admits(t(50), t(60), t(100)));
        assert!(!window_admits(t(50), t(0), t(40)));
        // absent bounds impose no limit
        assert!(window_admits(t(50), None, None));
        // unknown submission time can't be rejected
        assert!(window_admits(None, t(0), t(100)));
    }

    #[test]
    fn filter_eligible_combines_both_checks() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0);
        let b = block(t(0), t(100));
        let mut members = HashSet::new();
        members.insert(Uuid::from_u128(1)); // space 1 is a member

        let subs = vec![
            submission(1, t(50)),  // member, in window -> keep
            submission(2, t(50)),  // non-member -> drop
            submission(1, t(200)), // member, out of window -> drop
        ];
        let kept = filter_eligible(&b, SpaceKind::Dao, &members, subs, t(50).unwrap());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].space_id, Uuid::from_u128(1));
        assert_eq!(kept[0].submitted_at, t(50));
    }

    #[test]
    fn rolling_admits_until_the_decay_cutoff_not_one_frequency() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0).unwrap();
        let hour = 3600;
        // A ballot one frequency old is NOT expired any more — it decays instead
        // (scoring::recency_weight halves it), which is what stops a Rolling block
        // publishing an empty table between submissions.
        assert!(rolling_admits(Some(t(0)), Some(1), t(hour)));
        assert!(rolling_admits(Some(t(0)), Some(1), t(hour + 1)));
        // Still admitted just inside the cutoff (8 half-lives), dropped at/after it.
        let cutoff = (crate::scoring::ROLLING_MAX_HALF_LIVES as i64) * hour;
        assert!(rolling_admits(Some(t(0)), Some(1), t(cutoff - 1)));
        assert!(!rolling_admits(Some(t(0)), Some(1), t(cutoff)));
        assert!(!rolling_admits(Some(t(0)), Some(1), t(cutoff + hour)));
        // The cutoff scales with the frequency: 24h frequency -> 8 days.
        assert!(rolling_admits(Some(t(0)), Some(24), t(7 * 24 * hour)));
        assert!(!rolling_admits(Some(t(0)), Some(24), t(8 * 24 * hour)));
        // absent frequency imposes no limit
        assert!(rolling_admits(Some(t(0)), None, t(hour * 1000)));
        // a non-positive frequency can't define a cutoff
        assert!(rolling_admits(Some(t(0)), Some(0), t(hour * 1000)));
        // unknown submission time can't be rejected
        assert!(rolling_admits(None, Some(1), t(hour * 1000)));
    }

    #[test]
    fn filter_eligible_uses_rolling_admits_for_rolling_blocks() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0).unwrap();
        let hour = 3600;
        let b = rolling_block(Some(1)); // 1-hour frequency -> 8-hour cutoff
        let mut members = HashSet::new();
        members.insert(Uuid::from_u128(1));

        let subs = vec![
            submission(1, Some(t(0))),          // member, fresh -> keep
            submission(1, Some(t(-2 * hour))),  // member, aged but inside cutoff -> keep (decays)
            submission(1, Some(t(-20 * hour))), // member, past the cutoff -> drop
            submission(2, Some(t(0))),          // non-member -> drop
        ];
        let kept = filter_eligible(&b, SpaceKind::Dao, &members, subs, t(hour / 2));
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|s| s.space_id == Uuid::from_u128(1)));
        assert!(kept.iter().all(|s| s.submitted_at != Some(t(-20 * hour))));
    }
}
