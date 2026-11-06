//! The Rust twin of `trending.current_agenda_listings()` (migrations 0113, 0115, 0118; GEO-3488).
//!
//! `trending.refresh_agenda_listings()` has two halves. The first brings `trending.agenda_listings`
//! in step with what the Debate agendas name now: [`compute`] is that set. The second re-scores
//! entities through `refresh_entity_ranking_scores`, which writes the ranking tables and is not
//! ported here. `tests/fixtures/agenda_listings_golden.txt` is the SQL's own output (scratch
//! postgres:16) over a seeded grid; the test compares to it. Writing geo-db is a later cutover
//! step: today the SQL table stays the source and a run can only compare ([`count_mismatches`]).
//!
//! The `datetime_utc` fallback of `trending.value_date` converts a timestamp to a New York day.
//! That conversion stays in the database: the reader passes the finished day as [`DateValue::ny_day`].

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::NaiveDate;
use uuid::Uuid;

const TYPES: Uuid = Uuid::from_u128(0x8f151ba4_de20_4e3c_9cb4_99ddf96f48f1);
const DEBATE_AGENDA: Uuid = Uuid::from_u128(0x79e3757b_2630_492c_9d3b_d73a78d45893);
pub const DATE_PROP: Uuid = Uuid::from_u128(0x52665f3e_fb7d_48d5_8b6b_abb21b0d36db);
const BLOCKS: Uuid = Uuid::from_u128(0xbeaba5cb_a677_41a8_b353_77030613fc70);
const CLAIMS: Uuid = Uuid::from_u128(0xe614cce1_c4ce_4586_8304_fd1237119eb2);
const DEBATES: Uuid = Uuid::from_u128(0xb96bf701_a399_430d_a072_f5e910cdeda9);
const TOPICS: Uuid = Uuid::from_u128(0x806d52bc_27e9_4c91_93c0_57978b093351);
const COLLECTION_ITEM: Uuid = Uuid::from_u128(0xa99f9ce1_2ffa_4dac_8c61_f6310d46064a);
const CLAIM_TYPE: Uuid = Uuid::from_u128(0x96f859ef_a1ca_4b22_9372_c86ad58b694b);
const DEBATE_TYPE: Uuid = Uuid::from_u128(0xfd51f935_2063_4617_be39_7b672b23364c);

#[derive(Debug, Clone, Copy)]
pub struct Relation {
    pub from: Uuid,
    pub to: Uuid,
    pub type_id: Uuid,
    pub space: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub id: Uuid,
    pub dao: bool,
}

/// A Date-property row of `values`.
#[derive(Debug, Clone)]
pub struct DateValue {
    pub entity: Uuid,
    pub space: Uuid,
    pub datetime: Option<String>,
    pub date: Option<String>,
    /// `datetime_utc` read in New York, as a day, or None when the column is null.
    pub ny_day: Option<NaiveDate>,
}

/// One row of `trending.agenda_listings` (minus `written_at`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Listing {
    pub agenda_id: Uuid,
    pub space_id: Uuid,
    pub entity_id: Uuid,
    pub listed_on: NaiveDate,
    pub source: &'static str,
}

/// `^\d{4}-\d{2}-\d{2}` on the start of `s`, then `left(s, 10)::date`; None when the cast fails.
fn leading_day(s: &str) -> Option<Option<NaiveDate>> {
    let b = s.as_bytes();
    let shape = b.len() >= 10
        && b[..10].iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    shape.then(|| NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok())
}

/// `trending.value_date`: the DATETIME text's day, else the DATE text's, else the NY day. A text
/// that has the shape but is not a real date makes the whole value null (the SQL's EXCEPTION arm).
pub fn value_date(v: &DateValue) -> Option<NaiveDate> {
    if let Some(d) = v.datetime.as_deref().and_then(leading_day) {
        return d;
    }
    if let Some(d) = v.date.as_deref().and_then(leading_day) {
        return d;
    }
    v.ny_day
}

/// The rows `trending.current_agenda_listings()` returns.
pub fn compute(rels: &[Relation], spaces: &[Space], dates: &[DateValue]) -> BTreeSet<Listing> {
    let dao: HashSet<Uuid> = spaces.iter().filter(|s| s.dao).map(|s| s.id).collect();

    let mut day_of: HashMap<(Uuid, Uuid), Option<NaiveDate>> = HashMap::new();
    for v in dates {
        if dao.contains(&v.space) {
            let d = value_date(v);
            let e = day_of.entry((v.entity, v.space)).or_insert(None);
            *e = (*e).max(d);
        }
    }
    // Every agenda in a DAO space it is typed in, with its Date there (null Dates drop out).
    let agendas: HashMap<(Uuid, Uuid), NaiveDate> = rels
        .iter()
        .filter(|r| r.type_id == TYPES && r.to == DEBATE_AGENDA && dao.contains(&r.space))
        .filter_map(|r| {
            day_of
                .get(&(r.from, r.space))
                .copied()
                .flatten()
                .map(|d| ((r.from, r.space), d))
        })
        .collect();

    // Types relations, any space: which entities are claims and which debates.
    let mut is_claim: HashSet<Uuid> = HashSet::new();
    let mut is_debate: HashSet<Uuid> = HashSet::new();
    for r in rels.iter().filter(|r| r.type_id == TYPES) {
        if r.to == CLAIM_TYPE {
            is_claim.insert(r.from);
        }
        if r.to == DEBATE_TYPE {
            is_debate.insert(r.from);
        }
    }

    // The agenda's own Blocks, Claims, Debates and Topics relations, in its own space.
    let mut links: BTreeSet<(Uuid, Uuid, Uuid, Uuid)> = BTreeSet::new();
    // Items of each block, in the block's space.
    let mut items: HashMap<(Uuid, Uuid), Vec<Uuid>> = HashMap::new();
    for r in rels {
        if r.type_id == COLLECTION_ITEM {
            items.entry((r.from, r.space)).or_default().push(r.to);
        } else if matches!(r.type_id, BLOCKS | CLAIMS | DEBATES | TOPICS)
            && agendas.contains_key(&(r.from, r.space))
        {
            links.insert((r.from, r.space, r.type_id, r.to));
        }
    }

    let mut out = BTreeSet::new();
    for (agenda, space, type_id, to) in links {
        let listed_on = agendas[&(agenda, space)];
        let mut push = |entity: Uuid, source: &'static str| {
            out.insert(Listing {
                agenda_id: agenda,
                space_id: space,
                entity_id: entity,
                listed_on,
                source,
            });
        };
        match type_id {
            BLOCKS => {
                for item in items.get(&(to, space)).into_iter().flatten() {
                    if is_claim.contains(item) {
                        push(*item, "block_item");
                    }
                }
            }
            CLAIMS if is_claim.contains(&to) => push(to, "agenda_claims"),
            DEBATES if is_debate.contains(&to) => push(to, "agenda_debates"),
            TOPICS => push(to, "agenda_topics"),
            _ => {}
        }
    }
    out
}

/// How many rows are in one set and not the other.
pub fn count_mismatches(got: &BTreeSet<Listing>, want: &BTreeSet<Listing>) -> usize {
    got.symmetric_difference(want).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/agenda_listings_golden.txt");

    fn id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn opt(s: &str) -> Option<String> {
        (s != "~").then(|| s.to_string())
    }

    fn source(s: &str) -> &'static str {
        match s {
            "block_item" => "block_item",
            "agenda_claims" => "agenda_claims",
            "agenda_debates" => "agenda_debates",
            "agenda_topics" => "agenda_topics",
            other => panic!("unknown source {other}"),
        }
    }

    struct Fixture {
        rels: Vec<Relation>,
        spaces: Vec<Space>,
        dates: Vec<DateValue>,
        want: BTreeSet<Listing>,
    }

    fn load() -> Fixture {
        let mut f = Fixture {
            rels: vec![],
            spaces: vec![],
            dates: vec![],
            want: BTreeSet::new(),
        };
        for line in GOLDEN.lines().filter(|l| !l.is_empty()) {
            let p: Vec<&str> = line.split('|').collect();
            match p[0] {
                "R" => f.rels.push(Relation {
                    from: id(p[1]),
                    to: id(p[2]),
                    type_id: id(p[3]),
                    space: id(p[4]),
                }),
                "S" => f.spaces.push(Space {
                    id: id(p[1]),
                    dao: p[2] == "DAO",
                }),
                // Only Date-property rows matter; the noise rows are dropped like the SQL's join does.
                "V" if id(p[3]) == DATE_PROP => f.dates.push(DateValue {
                    entity: id(p[1]),
                    space: id(p[2]),
                    datetime: opt(p[4]),
                    date: opt(p[5]),
                    ny_day: opt(p[6]).map(|d| d.parse().unwrap()),
                }),
                "V" => {}
                "O" => {
                    f.want.insert(Listing {
                        agenda_id: id(p[1]),
                        space_id: id(p[2]),
                        entity_id: id(p[3]),
                        listed_on: p[4].parse().unwrap(),
                        source: source(p[5]),
                    });
                }
                other => panic!("unknown tag {other}"),
            }
        }
        f
    }

    #[test]
    fn matches_the_sql_over_the_grid() {
        let f = load();
        assert!(f.want.len() > 300, "grid too small: {}", f.want.len());
        let sources: BTreeSet<_> = f.want.iter().map(|l| l.source).collect();
        assert_eq!(sources.len(), 4, "every source must appear in the grid");
        let got = compute(&f.rels, &f.spaces, &f.dates);
        assert_eq!(got.len(), f.want.len());
        assert_eq!(count_mismatches(&got, &f.want), 0);
    }

    fn dv(datetime: Option<&str>, date: Option<&str>, ny: Option<&str>) -> DateValue {
        DateValue {
            entity: Uuid::nil(),
            space: Uuid::nil(),
            datetime: datetime.map(String::from),
            date: date.map(String::from),
            ny_day: ny.map(|d| d.parse().unwrap()),
        }
    }

    #[test]
    fn value_date_follows_the_sql_branches() {
        let d = |s: &str| Some(s.parse::<NaiveDate>().unwrap());
        assert_eq!(
            value_date(&dv(Some("2026-11-03T00:00:00Z"), Some("2026-10-01"), None)),
            d("2026-11-03")
        );
        assert_eq!(
            value_date(&dv(None, Some("2026-10-01"), None)),
            d("2026-10-01")
        );
        assert_eq!(
            value_date(&dv(Some("garbage"), None, Some("2026-09-09"))),
            d("2026-09-09")
        );
        // Right shape, not a date: the whole value is null, it does not fall through.
        assert_eq!(
            value_date(&dv(
                Some("2026-13-45T00:00:00Z"),
                Some("2026-10-01"),
                Some("2026-09-09")
            )),
            None
        );
        assert_eq!(value_date(&dv(None, None, None)), None);
    }

    #[test]
    fn mismatches_count_both_directions() {
        let f = load();
        let mut got = compute(&f.rels, &f.spaces, &f.dates);
        let first = got.iter().next().unwrap().clone();
        got.remove(&first);
        assert_eq!(count_mismatches(&got, &f.want), 1);
        assert_eq!(count_mismatches(&BTreeSet::new(), &f.want), f.want.len());
    }
}
