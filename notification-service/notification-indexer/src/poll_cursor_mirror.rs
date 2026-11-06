//! How a `notification_poll_cursors` row is written to geo-db's `cursors` collection (collection 10).
//!
//! The row is `(name, cursor_updated_at, cursor_id)`: a high-water mark into Postgres `votes_count`, not a
//! stream offset. It is mirrored beside the chain cursors with `geodb_cursor::CursorMirror`: key
//! `notification_poll:<name>`, value `<cursor_id>:<cursor_updated_at as RFC 3339 with microseconds, Z>`.
//! The mirror is best effort and off unless `GEODB_CURSOR_MIRROR` is on; Postgres stays the source of truth
//! and a mirror failure never fails the poller. This module is the pure part: the key and value shapes, and the
//! decoder a shadow read uses to compare.

use chrono::{DateTime, SecondsFormat, Utc};

/// Prefix that keeps poll cursors apart from the chain cursors in the same collection.
pub const KEY_PREFIX: &str = "notification_poll:";

/// The `cursors` key of a poll cursor.
pub fn mirror_key(name: &str) -> String {
    format!("{KEY_PREFIX}{name}")
}

/// The `(cursor, block)` arguments of `CursorMirror::mirror`: the timestamp text and the row id.
/// A negative id (never written today) is sent as 0.
pub fn mirror_parts(updated_at: DateTime<Utc>, id: i64) -> (String, u64) {
    (
        updated_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        id.max(0) as u64,
    )
}

/// Reads a stored value (`<id>:<timestamp>`) back to `(updated_at, id)`; `None` when it is not that shape.
pub fn parse_mirror_value(value: &str) -> Option<(DateTime<Utc>, i64)> {
    let (id, ts) = value.split_once(':')?;
    let id: i64 = id.parse().ok()?;
    let ts = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
    Some((ts, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn key_is_prefixed_so_it_cannot_meet_a_chain_cursor() {
        assert_eq!(
            mirror_key("entity_votes_threshold"),
            "notification_poll:entity_votes_threshold"
        );
        assert_eq!(
            mirror_key("proposal_rejected_floor"),
            "notification_poll:proposal_rejected_floor"
        );
    }

    #[test]
    fn value_is_id_then_microsecond_timestamp() {
        let (cursor, block) = mirror_parts(ts("2026-11-06T12:34:56.123456Z"), 42);
        assert_eq!(
            (cursor.as_str(), block),
            ("2026-11-06T12:34:56.123456Z", 42)
        );
        // The mirror joins them as "{block}:{cursor}".
        assert_eq!(
            format!("{block}:{cursor}"),
            "42:2026-11-06T12:34:56.123456Z"
        );
    }

    #[test]
    fn a_whole_second_still_prints_six_digits() {
        let (cursor, _) = mirror_parts(ts("2026-11-06T00:00:00Z"), 0);
        assert_eq!(cursor, "2026-11-06T00:00:00.000000Z");
    }

    #[test]
    fn negative_id_is_sent_as_zero() {
        assert_eq!(mirror_parts(ts("2026-01-01T00:00:00Z"), -3).1, 0);
    }

    #[test]
    fn value_round_trips_to_the_postgres_row() {
        for (t, id) in [
            ("2026-11-06T12:34:56.123456Z", 42),
            ("2026-11-06T00:00:00Z", 0),
            ("2020-02-29T23:59:59.999999Z", i64::MAX),
        ] {
            let (cursor, block) = mirror_parts(ts(t), id);
            assert_eq!(
                parse_mirror_value(&format!("{block}:{cursor}")),
                Some((ts(t), id))
            );
        }
    }

    #[test]
    fn a_malformed_value_does_not_parse() {
        for v in [
            "",
            "x",
            "1",
            "1:",
            ":2026-01-01T00:00:00Z",
            "a:2026-01-01T00:00:00Z",
            "1:yesterday",
        ] {
            assert_eq!(parse_mirror_value(v), None, "{v:?}");
        }
    }
}
