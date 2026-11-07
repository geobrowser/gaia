//! The Rust twin of the debate watch-score plpgsql (migrations 0111, 0112 and 0114): `debate_watch_scores(uuid[])`
//! and the selector inside `refresh_debate_watch_scores()` (zero-Postgres plan 10.22 item 5).
//!
//! * [`watch_scores`] is `debate_watch_scores`: for each Debate-typed entity among the ids, the signals summed over
//!   the last `watch_window_days`, and the clamped watch term plus the fresh-debate boost.
//! * [`refresh_ids`] is the selection `refresh_debate_watch_scores` makes before it calls
//!   `refresh_entity_ranking_scores`: every Debate with signals in the window, a non-zero stored `watch_score`, or a
//!   `created_at` inside the window.
//!
//! Not ported: `refresh_entity_ranking_scores`, which writes the scores (blocked on zero-Postgres GAP-3 and the
//! 10.14 section 11 decision). A shadow in `feed_signals_sync` (flag `DEBATE_WATCH_RUST_COMPUTE`, default off) runs
//! these against the same transaction and logs how they compare; it never writes.
//!
//! Time is an input, in microseconds since the epoch, because the SQL reads `now()`. The SQL subtracts
//! `make_interval(days => n)`, which is `n * 86400` seconds when the session time zone is UTC (a calendar day
//! elsewhere); the twin assumes UTC. `tests/fixtures/debate_watch_golden.txt` is the SQL's own output (scratch
//! postgres:16, the migrations' function text, the clock fixed at 2026-11-01 12:00:00 UTC, plus 0.123456 s in alternate scenarios, synthetic data).

use std::collections::{BTreeSet, HashMap, HashSet};

use sqlx::PgConnection;
use uuid::Uuid;

const TYPES: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);
const DAY_MICROS: i64 = 86_400 * 1_000_000;

/// The `watch_*` columns of `entity_ranking_config`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchConfig {
    pub window_days: i64,
    pub prior_impressions: f64,
    pub reference_seconds: f64,
    pub ratio_weight: f64,
    pub completed_weight: f64,
    pub plays_weight: f64,
    pub claim_votes_weight: f64,
    pub cap: f64,
    pub fresh_boost: f64,
    pub fresh_impressions: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Rel {
    pub from: Uuid,
    pub to: Uuid,
    pub type_id: Uuid,
}

/// One `personalization.feed_item_signals_hourly` row.
#[derive(Debug, Clone, Copy)]
pub struct Signal {
    pub item: Uuid,
    /// `hour` in microseconds since the epoch.
    pub hour_micros: i64,
    pub impressions: i64,
    pub plays: i64,
    pub explore_watch_ms: i64,
    pub completed_plays: i64,
    pub claim_votes: i64,
}

/// One `debate_watch_scores` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Watch {
    pub id: Uuid,
    pub impressions: i64,
    pub plays: i64,
    pub explore_watch_seconds: f64,
    pub completed_plays: i64,
    pub claim_votes: i64,
    pub watch_score: f64,
}

/// `CASE WHEN created_at ~ '^[0-9]+$' THEN created_at::bigint END`. A digit string too long for a bigint raises in
/// the SQL; here it is None.
pub fn created_epoch(created_at: Option<&str>) -> Option<i64> {
    let s = created_at?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The distinct entities typed Debate (`Types` -> `Debate`), among `ids` when given.
pub fn debate_ids(rels: &[Rel], ids: Option<&[Uuid]>) -> BTreeSet<Uuid> {
    let only: Option<HashSet<Uuid>> = ids.map(|i| i.iter().copied().collect());
    rels.iter()
        .filter(|r| r.type_id == TYPES && r.to == DEBATE_TYPE)
        .filter(|r| only.as_ref().is_none_or(|o| o.contains(&r.from)))
        .map(|r| r.from)
        .collect()
}

fn window_start(cfg: &WatchConfig, now_micros: i64) -> i64 {
    now_micros - cfg.window_days * DAY_MICROS
}

/// `created_epoch >= extract(epoch FROM now() - window)`, in microseconds so a fractional clock compares exactly.
fn created_in_window(created: Option<i64>, start_micros: i64) -> bool {
    created.is_some_and(|c| c.checked_mul(1_000_000).is_some_and(|m| m >= start_micros))
}

/// `debate_watch_scores(entity_ids)`, ordered by id. `created` maps an entity id to its `created_at` as parsed by
/// [`created_epoch`]; an id missing from it has no `entities` row and gets no fresh boost.
pub fn watch_scores(
    cfg: &WatchConfig,
    now_micros: i64,
    ids: &[Uuid],
    rels: &[Rel],
    signals: &[Signal],
    created: &HashMap<Uuid, Option<i64>>,
) -> Vec<Watch> {
    let start = window_start(cfg, now_micros);
    let debates = debate_ids(rels, Some(ids));
    let mut sums: HashMap<Uuid, (i64, i64, i64, i64, i64)> = HashMap::new();
    for s in signals {
        if debates.contains(&s.item) && s.hour_micros >= start {
            let t = sums.entry(s.item).or_default();
            t.0 += s.impressions;
            t.1 += s.plays;
            t.2 += s.explore_watch_ms;
            t.3 += s.completed_plays;
            t.4 += s.claim_votes;
        }
    }
    debates
        .into_iter()
        .map(|id| {
            let (impressions, plays, ms, completed_plays, claim_votes) =
                sums.get(&id).copied().unwrap_or_default();
            let explore_watch_seconds = ms as f64 / 1000.0;
            let fresh = created_in_window(created.get(&id).copied().flatten(), start);
            Watch {
                id,
                impressions,
                plays,
                explore_watch_seconds,
                completed_plays,
                claim_votes,
                watch_score: score(
                    cfg,
                    impressions,
                    plays,
                    explore_watch_seconds,
                    completed_plays,
                    claim_votes,
                    fresh,
                ),
            }
        })
        .collect()
}

fn score(
    cfg: &WatchConfig,
    impressions: i64,
    plays: i64,
    explore_watch_seconds: f64,
    completed_plays: i64,
    claim_votes: i64,
    fresh: bool,
) -> f64 {
    let imp = impressions.max(0) as f64;
    let ratio = (explore_watch_seconds.max(0.0) + cfg.prior_impressions * cfg.reference_seconds)
        / (imp + cfg.prior_impressions)
        / cfg.reference_seconds;
    let term = cfg.ratio_weight * ratio.ln()
        + cfg.completed_weight * (1.0 + completed_plays.max(0) as f64).ln()
        + cfg.plays_weight * (1.0 + plays.max(0) as f64).ln()
        + cfg.claim_votes_weight * (1.0 + claim_votes.max(0) as f64).ln();
    let clamped = term.min(cfg.cap).max(-cfg.cap);
    let boost = if fresh {
        cfg.fresh_boost * (1.0 - imp / cfg.fresh_impressions).max(0.0)
    } else {
        0.0
    };
    clamped + boost
}

/// The ids `refresh_debate_watch_scores()` hands to `refresh_entity_ranking_scores`, sorted (`array_agg(DISTINCT ..)`).
/// `stored_nonzero` is the set of entities whose stored `entity_ranking_scores.watch_score` is not 0. Empty means
/// the SQL returns 0 without calling the scorer.
pub fn refresh_ids(
    cfg: &WatchConfig,
    now_micros: i64,
    rels: &[Rel],
    signals: &[Signal],
    stored_nonzero: &HashSet<Uuid>,
    created: &HashMap<Uuid, Option<i64>>,
) -> Vec<Uuid> {
    let start = window_start(cfg, now_micros);
    let with_signals: HashSet<Uuid> = signals
        .iter()
        .filter(|s| s.hour_micros >= start)
        .map(|s| s.item)
        .collect();
    debate_ids(rels, None)
        .into_iter()
        .filter(|id| {
            with_signals.contains(id)
                || stored_nonzero.contains(id)
                || created_in_window(created.get(id).copied().flatten(), start)
        })
        .collect()
}

/// True when DEBATE_WATCH_RUST_COMPUTE is `1`, `true` or `yes`. Off by default.
pub fn shadow_enabled() -> bool {
    matches!(
        std::env::var("DEBATE_WATCH_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// What the selector and the scorer read, taken inside the sync transaction just before the SQL call.
pub struct Snapshot {
    cfg: WatchConfig,
    now_micros: i64,
    rels: Vec<Rel>,
    signals: Vec<Signal>,
    stored_nonzero: HashSet<Uuid>,
    created: HashMap<Uuid, Option<i64>>,
}

type ConfigRow = (i64, f64, f64, f64, f64, f64, f64, f64, f64, f64);
type SignalRow = (Uuid, i64, i64, i64, i64, i64, i64);

const NOW_SQL: &str = "SELECT (extract(epoch FROM now())::numeric * 1000000)::bigint";
const CONFIG_SQL: &str = "SELECT watch_window_days::bigint, watch_prior_impressions::float8, \
    watch_reference_seconds::float8, watch_ratio_weight::float8, watch_completed_weight::float8, \
    watch_plays_weight::float8, watch_claim_votes_weight::float8, watch_cap::float8, \
    watch_fresh_boost::float8, watch_fresh_impressions::float8 \
    FROM public.entity_ranking_config WHERE id";
const DEBATES_SQL: &str = "SELECT DISTINCT from_entity_id FROM public.relations \
    WHERE type_id = '8f151ba4-de20-4e3c-9cb4-99ddf96f48f1'::uuid \
      AND to_entity_id = 'fd51f935-2063-4617-be39-7b672b23364c'::uuid";
const SIGNALS_SQL: &str = "SELECT item_id, (extract(epoch FROM hour)::numeric * 1000000)::bigint, \
    impressions::bigint, plays::bigint, explore_watch_ms::bigint, completed_plays::bigint, \
    debate_claim_votes::bigint \
    FROM personalization.feed_item_signals_hourly WHERE item_id = ANY($1)";
const STORED_SQL: &str = "SELECT entity_id FROM public.entity_ranking_scores \
    WHERE entity_id = ANY($1) AND watch_score <> 0";
const CREATED_SQL: &str = "SELECT id, created_at FROM public.entities WHERE id = ANY($1)";
const SCORES_SQL: &str = "SELECT entity_id, watch_score::float8 FROM public.entity_ranking_scores \
    WHERE entity_id = ANY($1)";

async fn read_snapshot(conn: &mut PgConnection) -> Result<Snapshot, sqlx::Error> {
    let now_micros: i64 = sqlx::query_scalar(NOW_SQL).fetch_one(&mut *conn).await?;
    let c: ConfigRow = sqlx::query_as(CONFIG_SQL).fetch_one(&mut *conn).await?;
    let cfg = WatchConfig {
        window_days: c.0,
        prior_impressions: c.1,
        reference_seconds: c.2,
        ratio_weight: c.3,
        completed_weight: c.4,
        plays_weight: c.5,
        claim_votes_weight: c.6,
        cap: c.7,
        fresh_boost: c.8,
        fresh_impressions: c.9,
    };
    let debates: Vec<Uuid> = sqlx::query_scalar(DEBATES_SQL)
        .fetch_all(&mut *conn)
        .await?;
    let rels = debates
        .iter()
        .map(|&from| Rel {
            from,
            to: DEBATE_TYPE,
            type_id: TYPES,
        })
        .collect();
    let signals = sqlx::query_as::<_, SignalRow>(SIGNALS_SQL)
        .bind(&debates)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| Signal {
            item: r.0,
            hour_micros: r.1,
            impressions: r.2,
            plays: r.3,
            explore_watch_ms: r.4,
            completed_plays: r.5,
            claim_votes: r.6,
        })
        .collect();
    let stored_nonzero = sqlx::query_scalar(STORED_SQL)
        .bind(&debates)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect();
    let created = sqlx::query_as::<_, (Uuid, Option<String>)>(CREATED_SQL)
        .bind(&debates)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|(id, at)| (id, created_epoch(at.as_deref())))
        .collect();
    Ok(Snapshot {
        cfg,
        now_micros,
        rels,
        signals,
        stored_nonzero,
        created,
    })
}

/// Reads the inputs in a savepoint, so a failed read cannot poison the caller's transaction. None on any error.
pub async fn before(conn: &mut PgConnection) -> Option<Snapshot> {
    let mut sp = match sqlx::Acquire::begin(&mut *conn).await {
        Ok(sp) => sp,
        Err(e) => {
            tracing::warn!(error = %e, "debate watch rust compute skipped: no savepoint");
            return None;
        }
    };
    match read_snapshot(&mut sp).await {
        Ok(s) => {
            let _ = sp.commit().await;
            Some(s)
        }
        Err(e) => {
            let _ = sp.rollback().await;
            tracing::warn!(error = %e, "debate watch rust compute skipped: read failed");
            None
        }
    }
}

/// After `refresh_debate_watch_scores()` ran: logs whether the twin's selection size and the scores it left behind
/// agree with the twin. `rescored` is what the SQL returned (rows written, which needs an `entities` row).
pub async fn after(conn: &mut PgConnection, snap: Snapshot, rescored: i32) {
    let ids = refresh_ids(
        &snap.cfg,
        snap.now_micros,
        &snap.rels,
        &snap.signals,
        &snap.stored_nonzero,
        &snap.created,
    );
    // The SQL writes only entities that have a row.
    let scorable: Vec<Uuid> = ids
        .iter()
        .copied()
        .filter(|i| snap.created.contains_key(i))
        .collect();
    let rust = watch_scores(
        &snap.cfg,
        snap.now_micros,
        &scorable,
        &snap.rels,
        &snap.signals,
        &snap.created,
    );
    let stored = async {
        let mut sp = sqlx::Acquire::begin(&mut *conn).await?;
        let r = sqlx::query_as::<_, (Uuid, f64)>(SCORES_SQL)
            .bind(&scorable)
            .fetch_all(&mut *sp)
            .await;
        let _ = sp.rollback().await;
        r
    }
    .await;
    match stored {
        Err(e) => {
            tracing::warn!(error = %e, "debate watch rust compute skipped: stored scores unreadable")
        }
        Ok(rows) => {
            let stored: HashMap<Uuid, f64> = rows.into_iter().collect();
            let (compared, differ, missing) = compare_stored(&rust, &stored);
            if rescored as usize == scorable.len() && differ == 0 && missing == 0 {
                tracing::info!(
                    selected = ids.len(),
                    rescored,
                    compared,
                    "debate watch rust compute matches the SQL"
                );
            } else {
                tracing::warn!(
                    selected = ids.len(),
                    scorable = scorable.len(),
                    rescored,
                    compared,
                    differ,
                    missing,
                    "debate watch rust compute differs from the SQL"
                );
            }
        }
    }
}

/// Compares a twin's scores with the stored ones (`None` = no stored row). Returns (compared, differ, missing).
pub fn compare_stored(rust: &[Watch], stored: &HashMap<Uuid, f64>) -> (usize, usize, usize) {
    let (mut compared, mut differ, mut missing) = (0, 0, 0);
    for w in rust {
        match stored.get(&w.id) {
            None => missing += 1,
            Some(s) => {
                compared += 1;
                if !crate::entity_ranking_score_compute::close(w.watch_score, *s) {
                    differ += 1;
                }
            }
        }
    }
    (compared, differ, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/debate_watch_golden.txt");

    fn id(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    #[derive(Default)]
    struct Scenario {
        cfg: Option<WatchConfig>,
        rels: Vec<Rel>,
        signals: Vec<Signal>,
        created: HashMap<Uuid, Option<i64>>,
        stored: HashSet<Uuid>,
        watch: Vec<Watch>,
        two: Option<usize>,
        returned: Option<i32>,
        called: Vec<Uuid>,
        entities: Vec<Uuid>,
        now: i64,
    }

    fn scenarios() -> Vec<Scenario> {
        let mut out: Vec<Scenario> = Vec::new();
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            let i: usize = p[1].parse().unwrap();
            while out.len() <= i {
                out.push(Scenario::default());
            }
            let s = &mut out[i];
            match p[0] {
                "SC" => {
                    let f = |k: usize| p[k].parse::<f64>().unwrap();
                    s.cfg = Some(WatchConfig {
                        window_days: p[2].parse().unwrap(),
                        prior_impressions: f(3),
                        reference_seconds: f(4),
                        ratio_weight: f(5),
                        completed_weight: f(6),
                        plays_weight: f(7),
                        claim_votes_weight: f(8),
                        cap: f(9),
                        fresh_boost: f(10),
                        fresh_impressions: f(11),
                    });
                }
                "CLK" => s.now = p[2].parse().unwrap(),
                "R" => s.rels.push(Rel {
                    from: id(p[2]),
                    to: id(p[3]),
                    type_id: id(p[4]),
                }),
                "E" => {
                    let e = id(p[2]);
                    s.entities.push(e);
                    s.created.insert(e, created_epoch(Some(p[3])));
                }
                "H" => s.signals.push(Signal {
                    item: id(p[2]),
                    hour_micros: p[3].parse().unwrap(),
                    impressions: p[4].parse().unwrap(),
                    plays: p[5].parse().unwrap(),
                    explore_watch_ms: p[6].parse().unwrap(),
                    completed_plays: p[7].parse().unwrap(),
                    claim_votes: p[8].parse().unwrap(),
                }),
                "SCORE" => {
                    if p[3].parse::<f64>().unwrap() != 0.0 {
                        s.stored.insert(id(p[2]));
                    }
                }
                "OW" => s.watch.push(Watch {
                    id: id(p[2]),
                    impressions: p[3].parse().unwrap(),
                    plays: p[4].parse().unwrap(),
                    explore_watch_seconds: p[5].parse().unwrap(),
                    completed_plays: p[6].parse().unwrap(),
                    claim_votes: p[7].parse().unwrap(),
                    watch_score: p[8].parse().unwrap(),
                }),
                "OW2" => s.two = Some(p[2].parse().unwrap()),
                "OR" => s.returned = Some(p[2].parse().unwrap()),
                "OI" => s.called = p[2].split(',').filter(|x| !x.is_empty()).map(id).collect(),
                other => panic!("unknown tag {other}"),
            }
        }
        out
    }

    fn all_ids() -> Vec<Uuid> {
        (0..30u64)
            .map(|k| {
                Uuid::from_u128(
                    (0x5u128 << 96) | (0x4000u128 << 64) | (0x8000u128 << 48) | k as u128,
                )
            })
            .collect()
    }

    #[test]
    fn fixture_shape() {
        let s = scenarios();
        assert_eq!(s.len(), 12);
        assert!(s.iter().all(|s| s.cfg.is_some() && !s.watch.is_empty()));
        assert!(
            s.iter().any(|s| s.called.is_empty()),
            "a scenario where nothing is selected"
        );
        assert!(s.iter().filter(|s| !s.called.is_empty()).count() >= 10);
        let boosted = s
            .iter()
            .flat_map(|s| s.watch.iter())
            .filter(|w| w.watch_score.fract() != 0.0)
            .count();
        assert!(boosted > 50);
        assert!(s.iter().all(|s| s.two == Some(0)));
    }

    #[test]
    fn watch_scores_match_the_sql() {
        let ids = all_ids();
        for (n, s) in scenarios().iter().enumerate() {
            let got = watch_scores(
                &s.cfg.unwrap(),
                s.now,
                &ids,
                &s.rels,
                &s.signals,
                &s.created,
            );
            assert_eq!(got.len(), s.watch.len(), "scenario {n}: debates");
            for (g, w) in got.iter().zip(&s.watch) {
                assert_eq!(
                    (
                        g.id,
                        g.impressions,
                        g.plays,
                        g.completed_plays,
                        g.claim_votes
                    ),
                    (
                        w.id,
                        w.impressions,
                        w.plays,
                        w.completed_plays,
                        w.claim_votes
                    ),
                    "scenario {n}"
                );
                assert_eq!(
                    g.explore_watch_seconds, w.explore_watch_seconds,
                    "scenario {n}"
                );
                assert!(
                    crate::entity_ranking_score_compute::close(g.watch_score, w.watch_score),
                    "scenario {n} {}: {} vs {}",
                    g.id,
                    g.watch_score,
                    w.watch_score
                );
            }
            assert!(
                watch_scores(&s.cfg.unwrap(), s.now, &[], &s.rels, &s.signals, &s.created)
                    .is_empty()
            );
        }
    }

    #[test]
    fn refresh_ids_match_the_sql() {
        for (n, s) in scenarios().iter().enumerate() {
            let got = refresh_ids(
                &s.cfg.unwrap(),
                s.now,
                &s.rels,
                &s.signals,
                &s.stored,
                &s.created,
            );
            assert_eq!(got, s.called, "scenario {n}");
            assert_eq!(got.len() as i32, s.returned.unwrap(), "scenario {n}");
        }
    }

    #[test]
    fn created_epoch_is_digits_only() {
        assert_eq!(created_epoch(Some("123")), Some(123));
        for bad in ["", " 5", "-5", "12x", "n/a", "99999999999999999999"] {
            assert_eq!(created_epoch(Some(bad)), None, "{bad:?}");
        }
        assert_eq!(created_epoch(None), None);
    }

    #[test]
    fn compare_counts_missing_and_differing() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let w = |id, s| Watch {
            id,
            impressions: 0,
            plays: 0,
            explore_watch_seconds: 0.0,
            completed_plays: 0,
            claim_votes: 0,
            watch_score: s,
        };
        let stored = HashMap::from([(a, 1.0), (b, 2.5)]);
        assert_eq!(
            compare_stored(&[w(a, 1.0 + 1e-12), w(b, 2.0), w(c, 0.0)], &stored),
            (2, 1, 1)
        );
    }
}
