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
/// Valid while `now - submitted_at < submission_frequency` (hours); once a
/// submission ages past its frequency it requires resubmission. `now` is a
/// parameter (never read via an inline `Utc::now()` call here) so callers —
/// tests and the periodic sweep alike — control it deterministically.
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
    now - submitted_at < chrono::Duration::hours(frequency_hours.into())
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
    fn rolling_admits_window() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0).unwrap();
        let hour = 3600;
        // within: 1 hour old, 2-hour frequency
        assert!(rolling_admits(Some(t(0)), Some(2), t(hour)));
        // exactly-at-boundary: elapsed == frequency is expired, not admitted
        assert!(!rolling_admits(Some(t(0)), Some(1), t(hour)));
        // just past the boundary
        assert!(!rolling_admits(Some(t(0)), Some(1), t(hour + 1)));
        // just before the boundary
        assert!(rolling_admits(Some(t(0)), Some(1), t(hour - 1)));
        // absent frequency imposes no limit
        assert!(rolling_admits(Some(t(0)), None, t(hour * 1000)));
        // unknown submission time can't be rejected
        assert!(rolling_admits(None, Some(1), t(hour * 1000)));
    }

    #[test]
    fn filter_eligible_uses_rolling_admits_for_rolling_blocks() {
        let t = |n: i64| DateTime::<Utc>::from_timestamp(n, 0).unwrap();
        let hour = 3600;
        let b = rolling_block(Some(1)); // 1-hour frequency
        let mut members = HashSet::new();
        members.insert(Uuid::from_u128(1));

        let subs = vec![
            submission(1, Some(t(0))),         // member, still within frequency -> keep
            submission(1, Some(t(-2 * hour))), // member, expired -> drop
            submission(2, Some(t(0))),         // non-member -> drop
        ];
        let kept = filter_eligible(&b, SpaceKind::Dao, &members, subs, t(hour / 2));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].submitted_at, Some(t(0)));
    }
}
