//! The window `new_entity_sweep` scans, kept here so it can be tested without a database.

/// How far back the hourly run looks when nothing overrides it.
///
/// Two days rather than one hour so a run can fail (or the CronJob can be down) for a day
/// and the next success still covers everything created meanwhile. Wider windows cost more
/// every hour, because the anti-join probes every entity in the window, scored or not:
/// 2 days was 4.4s on the live DB, 30 days about 80s.
pub const DEFAULT_LOOKBACK_HOURS: u64 = 48;

/// Parse `NEW_ENTITY_SWEEP_LOOKBACK_HOURS`. Unset or blank means the default; anything
/// else must be a positive whole number of hours, and is an error otherwise, so a typo in a
/// one-off catch-up run fails loudly instead of silently sweeping two days.
pub fn lookback_hours(raw: Option<&str>) -> Result<u64, String> {
    match raw.map(str::trim) {
        None | Some("") => Ok(DEFAULT_LOOKBACK_HOURS),
        Some(value) => match value.parse::<u64>() {
            Ok(hours) if hours > 0 => Ok(hours),
            _ => Err(format!(
                "NEW_ENTITY_SWEEP_LOOKBACK_HOURS must be a positive whole number of hours, got {value:?}"
            )),
        },
    }
}

/// The earliest `created_at` a run considers, as the text the column holds.
///
/// `entities.created_at` is epoch seconds stored as text, and the query compares it as text
/// so it can use `entities_created_at_id_idx`. That ordering is only numeric while every
/// value has the same number of digits — true for 10-digit epochs from 2001 to 2286 —
/// so the cutoff is zero-padded to 10 digits rather than trusted to happen to be.
pub fn cutoff(now_epoch_secs: u64, lookback_hours: u64) -> String {
    let since = now_epoch_secs.saturating_sub(lookback_hours.saturating_mul(3600));
    format!("{since:010}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_blank_is_the_default() {
        assert_eq!(lookback_hours(None), Ok(DEFAULT_LOOKBACK_HOURS));
        assert_eq!(lookback_hours(Some("")), Ok(DEFAULT_LOOKBACK_HOURS));
        assert_eq!(lookback_hours(Some("  ")), Ok(DEFAULT_LOOKBACK_HOURS));
    }

    #[test]
    fn a_catch_up_window_parses() {
        assert_eq!(lookback_hours(Some("1200")), Ok(1200));
        assert_eq!(lookback_hours(Some(" 72 ")), Ok(72));
    }

    #[test]
    fn nonsense_is_an_error_not_the_default() {
        for bad in ["0", "-5", "48h", "2.5", "two days"] {
            assert!(
                lookback_hours(Some(bad)).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn cutoff_is_the_lookback_before_now() {
        // 2026-09-29T19:27:29Z, a real created_at from the live DB.
        assert_eq!(cutoff(1_790_710_049, 48), "1790537249");
    }

    #[test]
    fn cutoff_is_always_ten_digits_so_text_order_is_numeric_order() {
        assert_eq!(cutoff(5_000, 1), "0000001400");
        assert_eq!(cutoff(1_790_710_049, 1_000_000), "0000000000");
        // A cutoff compared as text must sort where its number does against a live value.
        assert!(cutoff(1_790_710_049, 48).as_str() < "1790710049");
        assert!(cutoff(1_790_710_049, 48).as_str() > "1790537248");
    }
}
