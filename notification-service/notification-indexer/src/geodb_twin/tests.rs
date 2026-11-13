use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

fn u(n: u128) -> Uuid {
    // Version-4 shaped ids pass through the conformer unchanged, which keeps fixtures readable.
    Uuid::from_u128((n << 64) | (4u128 << 76) | (0b10u128 << 62) | n)
}

const GEO_SPACE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GOV: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VOTES: &str = "cccccccccccccccccccccccccccccccc";

fn spaces(space: &Uuid) -> Spaces {
    let mut by = HashMap::new();
    by.insert(space.simple().to_string(), GEO_SPACE.to_string());
    let mut sys = HashMap::new();
    sys.insert("governance".to_string(), GOV.to_string());
    sys.insert("votes".to_string(), VOTES.to_string());
    Spaces::new(by).with_system(sys)
}

fn ent(id: &Uuid) -> String {
    conforming_id("entity", id)
}

/// A card row: property, value, statement, owner.
fn row(prop: &str, value: &str, owner: &str) -> String {
    format!("{prop}\t{value}\tstmt\t{owner}\n")
}

/// A backlink row: property, the referenced entity, statement, the referring entity.
fn link(prop: &str, to: &str, from: &str) -> String {
    format!("{prop}\t{to}\tstmt\t{from}\n")
}

#[derive(Default)]
struct Fake {
    answers: Mutex<HashMap<(String, String), String>>,
    fail: bool,
}

impl Fake {
    fn card(self, space: &str, entity: &str, body: String) -> Self {
        self.answers
            .lock()
            .unwrap()
            .insert((space.into(), card_doc(entity, None)), body);
        self
    }
    fn backlinks(self, space: &str, entity: &str, body: String) -> Self {
        self.answers
            .lock()
            .unwrap()
            .insert((space.into(), card_doc(entity, Some(MAX_BACKLINKS))), body);
        self
    }
    fn cards(self, space: &str, ids: &[String], body: String) -> Self {
        self.answers
            .lock()
            .unwrap()
            .insert((space.into(), cards_doc(ids)), body);
        self
    }
    fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }
}

#[async_trait]
impl GeoDbReader for Fake {
    async fn query(&self, spaces: &[String], doc: &str) -> Result<String, TwinError> {
        if self.fail {
            return Err(TwinError::Refused("fixture".into()));
        }
        Ok(self
            .answers
            .lock()
            .unwrap()
            .get(&(spaces[0].clone(), doc.to_string()))
            .cloned()
            .unwrap_or_default())
    }
}

// ---- editors and membership ------------------------------------------------------------------

fn space_card(space: &Uuid, editors: &[Uuid], members: &[Uuid]) -> Fake {
    let own = ent(space);
    let mut body = String::new();
    for e in editors {
        body += &row(EDITOR_PROPERTY, &ent(e), &own);
    }
    for m in members {
        body += &row(MEMBER_PROPERTY, &ent(m), &own);
    }
    Fake::default().card(GEO_SPACE, &own, body)
}

#[tokio::test]
async fn editors_match_regardless_of_order() {
    let (s, a, b) = (u(1), u(2), u(3));
    let fake = space_card(&s, &[a, b], &[]);
    let v = compare_editors(&fake, &spaces(&s), &s, &[b, a]).await;
    assert_eq!(v, vec![Verdict::Match]);
}

#[tokio::test]
async fn editors_missing_extra_and_both() {
    let (s, a, b, c) = (u(1), u(2), u(3), u(4));
    let sp = spaces(&s);
    let fake = space_card(&s, &[a], &[]);
    assert_eq!(
        compare_editors(&fake, &sp, &s, &[a, b]).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    let fake = space_card(&s, &[a, b], &[]);
    assert_eq!(
        compare_editors(&fake, &sp, &s, &[a]).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
    let fake = space_card(&s, &[a, c], &[]);
    assert_eq!(
        compare_editors(&fake, &sp, &s, &[a, b]).await,
        vec![Verdict::Mismatch("differs")]
    );
}

#[tokio::test]
async fn editors_ignore_member_edges_and_empty_matches_empty() {
    let (s, a, m) = (u(1), u(2), u(3));
    let fake = space_card(&s, &[a], &[m]);
    assert_eq!(
        compare_editors(&fake, &spaces(&s), &s, &[a]).await,
        vec![Verdict::Match]
    );
    let none = space_card(&s, &[], &[]);
    assert_eq!(
        compare_editors(&none, &spaces(&s), &s, &[]).await,
        vec![Verdict::Match]
    );
}

#[tokio::test]
async fn editors_unmapped_space_is_unsupported_and_read_failure_is_failed() {
    let (s, other) = (u(1), u(9));
    let sp = spaces(&s);
    let fake = Fake::default();
    assert_eq!(
        compare_editors(&fake, &sp, &other, &[]).await,
        vec![Verdict::Unsupported("space_not_loaded")]
    );
    assert_eq!(
        compare_editors(&Fake::failing(), &sp, &s, &[]).await,
        vec![Verdict::Failed]
    );
}

#[tokio::test]
async fn member_or_editor_checks_both_edge_kinds() {
    let (s, ed, mem, nobody) = (u(1), u(2), u(3), u(4));
    let fake = space_card(&s, &[ed], &[mem]);
    let sp = spaces(&s);
    for (user, sql, want) in [
        (ed, true, Verdict::Match),
        (mem, true, Verdict::Match),
        (nobody, false, Verdict::Match),
        (nobody, true, Verdict::Mismatch("missing_in_geodb")),
        (mem, false, Verdict::Mismatch("extra_in_geodb")),
    ] {
        assert_eq!(
            compare_member_or_editor(&fake, &sp, &s, &user, sql).await,
            vec![want]
        );
    }
}

// ---- voters ----------------------------------------------------------------------------------

fn votes_fixture(proposal: &Uuid, voters: &[Uuid]) -> Fake {
    let pe = ent(proposal);
    let records: Vec<String> = (0..voters.len())
        .map(|i| format!("{:032x}", 0x1000 + i as u128))
        .collect();
    let mut links = String::new();
    let mut cards = String::new();
    for (rec, voter) in records.iter().zip(voters) {
        links += &link(VOTE_PROPOSAL, &pe, rec);
        cards += &row(VOTE_VOTER, &ent(voter), rec);
        cards += &row(VOTE_PROPOSAL, &pe, rec);
    }
    let mut sorted = records.clone();
    sorted.sort();
    sorted.dedup();
    Fake::default()
        .backlinks(VOTES, &pe, links)
        .cards(VOTES, &sorted, cards)
}

#[tokio::test]
async fn voters_match_and_mismatch() {
    let (s, p, a, b, c) = (u(1), u(10), u(2), u(3), u(4));
    let sp = spaces(&s);
    let fake = votes_fixture(&p, &[a, b]);
    assert_eq!(
        compare_voters(&fake, &sp, &p, &[b, a]).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_voters(&fake, &sp, &p, &[a, b, c]).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    assert_eq!(
        compare_voters(&fake, &sp, &p, &[a]).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
}

#[tokio::test]
async fn voters_none_on_either_side_match() {
    let (s, p) = (u(1), u(10));
    let fake = Fake::default();
    assert_eq!(
        compare_voters(&fake, &spaces(&s), &p, &[]).await,
        vec![Verdict::Match]
    );
}

#[tokio::test]
async fn voters_unsupported_without_system_space_or_with_too_many_links() {
    let (s, p) = (u(1), u(10));
    let mut by = HashMap::new();
    by.insert(s.simple().to_string(), GEO_SPACE.to_string());
    let bare = Spaces::new(by);
    assert_eq!(
        compare_voters(&Fake::default(), &bare, &p, &[]).await,
        vec![Verdict::Unsupported("system_space_missing")]
    );
    let pe = ent(&p);
    let mut body = String::new();
    for i in 0..MAX_BACKLINKS {
        body += &link(VOTE_PROPOSAL, &pe, &format!("{:032x}", i + 1));
    }
    let fake = Fake::default().backlinks(VOTES, &pe, body);
    assert_eq!(
        compare_voters(&fake, &spaces(&s), &p, &[]).await,
        vec![Verdict::Unsupported("too_many_links")]
    );
    assert_eq!(
        compare_voters(&Fake::failing(), &spaces(&s), &p, &[]).await,
        vec![Verdict::Failed]
    );
}

// ---- proposal existence ----------------------------------------------------------------------

#[tokio::test]
async fn proposal_existence_follows_the_version_record() {
    let (s, p) = (u(1), u(10));
    let pe = ent(&p);
    let rec = "dddddddddddddddddddddddddddddddd";
    let held = Fake::default().backlinks(GOV, &pe, link(VERSION_PROPOSAL, &pe, rec));
    let sp = spaces(&s);
    assert_eq!(
        compare_proposal_exists(&held, &sp, &p, true).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_proposal_exists(&held, &sp, &p, false).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
    let none = Fake::default();
    assert_eq!(
        compare_proposal_exists(&none, &sp, &p, false).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_proposal_exists(&none, &sp, &p, true).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    // A backlink under another property (a status record) does not count as a version.
    let status_only = Fake::default().backlinks(GOV, &pe, link(STATUS_PROPOSAL, &pe, rec));
    assert_eq!(
        compare_proposal_exists(&status_only, &sp, &p, true).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
}

// ---- proposer and owning Space values --------------------------------------------------------

/// One version record per `(record, proposer, space)` for the proposal; each field is written only when `Some`.
fn version_values_fixture(p: &Uuid, versions: &[(&str, Option<&Uuid>, Option<&Uuid>)]) -> Fake {
    let pe = ent(p);
    let mut links = String::new();
    let mut cards = String::new();
    let mut ids: Vec<String> = vec![];
    for (rec, proposer, space) in versions {
        links += &link(VERSION_PROPOSAL, &pe, rec);
        if let Some(x) = proposer {
            cards += &row(VERSION_PROPOSER, &ent(x), rec);
        }
        if let Some(x) = space {
            cards += &row(VERSION_SPACE, &ent(x), rec);
        }
        ids.push((*rec).to_string());
    }
    ids.sort();
    Fake::default()
        .backlinks(GOV, &pe, links)
        .cards(GOV, &ids, cards)
}

const R1: &str = "dddddddddddddddddddddddddddddddd";
const R2: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

async fn values(fake: &Fake, sql: Option<ProposalValues>) -> Vec<Verdict> {
    compare_proposal_values(fake, &spaces(&u(1)), &u(10), sql).await
}

#[tokio::test]
async fn proposer_and_space_values_match_when_every_version_names_them() {
    let (p, proposer, space) = (u(10), u(20), u(30));
    let fake = version_values_fixture(
        &p,
        &[
            (R1, Some(&proposer), Some(&space)),
            (R2, Some(&proposer), Some(&space)),
        ],
    );
    let both = ProposalValues {
        proposer,
        space: Some(space),
    };
    assert_eq!(values(&fake, Some(both)).await, vec![Verdict::Match]);
    // The proposer-only read does not look at the Space.
    let only = ProposalValues {
        proposer,
        space: None,
    };
    assert_eq!(values(&fake, Some(only)).await, vec![Verdict::Match]);
}

#[tokio::test]
async fn proposer_and_space_values_mismatch_by_which_differs() {
    let (p, proposer, space, other) = (u(10), u(20), u(30), u(40));
    let fake = version_values_fixture(&p, &[(R1, Some(&proposer), Some(&space))]);
    let sql = |proposer, space| Some(ProposalValues { proposer, space });
    assert_eq!(
        values(&fake, sql(other, Some(space))).await,
        vec![Verdict::Mismatch("proposer_differs")]
    );
    assert_eq!(
        values(&fake, sql(proposer, Some(other))).await,
        vec![Verdict::Mismatch("space_differs")]
    );
    // A proposer difference with the Space unread still counts; a Space difference is not seen
    // by the proposer-only read.
    assert_eq!(
        values(&fake, sql(other, None)).await,
        vec![Verdict::Mismatch("proposer_differs")]
    );
    assert_eq!(
        values(&fake, sql(proposer, None)).await,
        vec![Verdict::Match]
    );
    // Two versions that disagree with each other cannot both equal Postgres's one value.
    let split = version_values_fixture(
        &p,
        &[
            (R1, Some(&proposer), Some(&space)),
            (R2, Some(&other), Some(&space)),
        ],
    );
    assert_eq!(
        values(&split, sql(proposer, Some(space))).await,
        vec![Verdict::Mismatch("proposer_differs")]
    );
    // A mismatch outranks an unsupported check.
    let proposer_wrong_space_unstored = version_values_fixture(&p, &[(R1, Some(&other), None)]);
    assert_eq!(
        values(&proposer_wrong_space_unstored, sql(proposer, Some(space))).await,
        vec![Verdict::Mismatch("proposer_differs")]
    );
}

#[tokio::test]
async fn proposer_and_space_values_not_stored_is_unsupported_not_a_mismatch() {
    let (p, proposer, space) = (u(10), u(20), u(30));
    let fake = version_values_fixture(&p, &[(R1, None, None)]);
    let sql = Some(ProposalValues {
        proposer,
        space: Some(space),
    });
    assert_eq!(
        values(&fake, sql).await,
        vec![Verdict::Unsupported("values_not_stored")]
    );
    // One stored and matching, the other not stored: unsupported, since the Space cannot be judged.
    let half = version_values_fixture(&p, &[(R1, Some(&proposer), None)]);
    assert_eq!(
        values(&half, sql).await,
        vec![Verdict::Unsupported("values_not_stored")]
    );
}

#[tokio::test]
async fn proposer_and_space_values_follow_existence_around_the_edges() {
    let (p, proposer) = (u(10), u(20));
    let sql = Some(ProposalValues {
        proposer,
        space: None,
    });
    // A row in Postgres and no version record.
    assert_eq!(
        values(&Fake::default(), sql).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    // No row in Postgres: the existence rules.
    assert_eq!(values(&Fake::default(), None).await, vec![Verdict::Match]);
    let held = version_values_fixture(&p, &[(R1, Some(&proposer), None)]);
    assert_eq!(
        values(&held, None).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
    // No governance Space, a failed read.
    let no_gov = Spaces::new(HashMap::new());
    assert_eq!(
        compare_proposal_values(&held, &no_gov, &p, sql).await,
        vec![Verdict::Unsupported("system_space_missing")]
    );
    assert_eq!(values(&Fake::failing(), sql).await, vec![Verdict::Failed]);
}

// ---- expiry scan -----------------------------------------------------------------------------

const NOW: i64 = 2_000;
const FLOOR: i64 = 1_000;

struct Proposal {
    versions: Vec<(i64, i64)>,
    executed: Option<&'static str>,
    escalation: bool,
}

fn gov_fixture(p: &Uuid, spec: &Proposal) -> Fake {
    let pe = ent(p);
    let mut links = String::new();
    let mut cards = String::new();
    let mut ids = vec![];
    for (i, (block, end)) in spec.versions.iter().enumerate() {
        let rec = format!("{:032x}", 0x2000 + i as u128);
        links += &link(VERSION_PROPOSAL, &pe, &rec);
        cards += &row(VERSION_BLOCK, &block.to_string(), &rec);
        cards += &row(VERSION_END_TIME, &format!("{end}.0"), &rec);
        ids.push(rec);
    }
    let status = format!("{:032x}", 0x3000);
    links += &link(STATUS_PROPOSAL, &pe, &status);
    if let Some(at) = spec.executed {
        cards += &row(STATUS_EXECUTED_AT, at, &status);
    }
    ids.push(status);
    if spec.escalation {
        links += &link(ESCALATION_PROPOSAL, &pe, &format!("{:032x}", 0x4000));
    }
    ids.sort();
    Fake::default()
        .backlinks(GOV, &pe, links)
        .cards(GOV, &ids, cards)
}

fn plain(versions: Vec<(i64, i64)>) -> Proposal {
    Proposal {
        versions,
        executed: None,
        escalation: false,
    }
}

async fn expiry(spec: Proposal, sql_end: i64) -> Vec<Verdict> {
    let (s, p) = (u(1), u(10));
    let fake = gov_fixture(&p, &spec);
    compare_expired_proposals(&fake, &spaces(&s), NOW, FLOOR, &[(p, sql_end)]).await
}

#[tokio::test]
async fn expiry_matches_when_the_latest_version_ended_unexecuted() {
    assert_eq!(
        expiry(plain(vec![(5, 1500)]), 1500).await,
        vec![Verdict::Match]
    );
    // The later block's end time is the one in force.
    assert_eq!(
        expiry(plain(vec![(5, 5000), (9, 1500)]), 1500).await,
        vec![Verdict::Match]
    );
}

#[tokio::test]
async fn expiry_mismatches() {
    assert_eq!(
        expiry(plain(vec![(5, 1400)]), 1500).await,
        vec![Verdict::Mismatch("end_time_differs")]
    );
    assert_eq!(
        expiry(plain(vec![(5, 3000)]), 3000).await,
        vec![Verdict::Mismatch("not_expired_in_geodb")]
    );
    assert_eq!(
        expiry(plain(vec![(5, 500)]), 500).await,
        vec![Verdict::Mismatch("not_expired_in_geodb")]
    );
    assert_eq!(
        expiry(
            Proposal {
                executed: Some("1700"),
                ..plain(vec![(5, 1500)])
            },
            1500
        )
        .await,
        vec![Verdict::Mismatch("executed_in_geodb")]
    );
    assert_eq!(
        expiry(plain(vec![]), 1500).await,
        vec![Verdict::Mismatch("no_version_record")]
    );
}

#[tokio::test]
async fn expiry_unsupported_cases() {
    assert_eq!(
        expiry(
            Proposal {
                escalation: true,
                ..plain(vec![(5, 1500)])
            },
            1500
        )
        .await,
        vec![Verdict::Unsupported("escalation_pending")]
    );
    // Two end times on one version record: the record cannot decide.
    let (s, p) = (u(1), u(10));
    let pe = ent(&p);
    let rec = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let fake = Fake::default()
        .backlinks(GOV, &pe, link(VERSION_PROPOSAL, &pe, rec))
        .cards(
            GOV,
            &[rec.to_string()],
            row(VERSION_BLOCK, "1", rec)
                + &row(VERSION_END_TIME, "1500", rec)
                + &row(VERSION_END_TIME, "1600", rec),
        );
    assert_eq!(
        compare_expired_proposals(&fake, &spaces(&s), NOW, FLOOR, &[(p, 1500)]).await,
        vec![Verdict::Unsupported("ambiguous_record")]
    );
    let mut by = HashMap::new();
    by.insert(s.simple().to_string(), GEO_SPACE.to_string());
    assert_eq!(
        compare_expired_proposals(
            &Fake::default(),
            &Spaces::new(by),
            NOW,
            FLOOR,
            &[(p, 1), (p, 2)]
        )
        .await,
        vec![Verdict::Unsupported("system_space_missing"); 2]
    );
}

#[tokio::test]
async fn expiry_failure_stops_the_run() {
    let s = u(1);
    let v = compare_expired_proposals(
        &Fake::failing(),
        &spaces(&s),
        NOW,
        FLOOR,
        &[(u(10), 1500), (u(11), 1500)],
    )
    .await;
    assert_eq!(v, vec![Verdict::Failed]);
}

// ---- config, counters, pins ------------------------------------------------------------------

fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k| m.get(k).cloned()
}

#[test]
fn flag_defaults_off_and_needs_its_paths() {
    assert!(TwinConfig::from_vars(vars(&[])).unwrap().is_none());
    assert!(TwinConfig::from_vars(vars(&[(FLAG, "0")]))
        .unwrap()
        .is_none());
    assert!(TwinConfig::from_vars(vars(&[(FLAG, "yes")]))
        .unwrap()
        .is_none());
    // Paths set but flag off: still off.
    assert!(TwinConfig::from_vars(vars(&[("GEODB_KEY_PATH", "/k")]))
        .unwrap()
        .is_none());
    let err = TwinConfig::from_vars(vars(&[(FLAG, "1")])).unwrap_err();
    assert!(err.contains("GEODB_KEY_PATH"), "{err}");
    let err = TwinConfig::from_vars(vars(&[
        (FLAG, "1"),
        ("GEODB_KEY_PATH", "/k"),
        ("GEODB_BINDING_PATH", "/b"),
    ]))
    .unwrap_err();
    assert!(err.contains("GEODB_SPACE_MAP_PATH"), "{err}");
}

#[test]
fn flag_on_reads_paths_binary_and_timeout() {
    let base = [
        (FLAG, "true"),
        ("GEODB_KEY_PATH", "/k"),
        ("GEODB_BINDING_PATH", "/b"),
        ("GEODB_SPACE_MAP_PATH", "/m/space-map.json"),
    ];
    let cfg = TwinConfig::from_vars(vars(&base)).unwrap().unwrap();
    assert_eq!(cfg.pipe.program, std::ffi::OsString::from("geodb-client"));
    assert_eq!(cfg.pipe.timeout, DEFAULT_TIMEOUT);
    assert_eq!(cfg.pipe.args.last().unwrap(), "pipe");
    assert_eq!(cfg.space_map, PathBuf::from("/m/space-map.json"));

    let mut more = base.to_vec();
    more.push(("GEODB_CLIENT_BIN", "/opt/geodb-client"));
    more.push((TIMEOUT_VAR, "250"));
    let cfg = TwinConfig::from_vars(vars(&more)).unwrap().unwrap();
    assert_eq!(
        cfg.pipe.program,
        std::ffi::OsString::from("/opt/geodb-client")
    );
    assert_eq!(cfg.pipe.timeout, Duration::from_millis(250));

    let mut bad = base.to_vec();
    bad.push((TIMEOUT_VAR, "0"));
    assert!(TwinConfig::from_vars(vars(&bad)).is_err());
}

#[test]
fn counter_family_labels_every_result() {
    assert_eq!(Verdict::Match.labels(), ("match", "none"));
    assert_eq!(
        Verdict::Mismatch("differs").labels(),
        ("mismatch", "differs")
    );
    assert_eq!(Verdict::Unsupported("x").labels(), ("unsupported", "x"));
    assert_eq!(Verdict::Failed.labels(), ("failed", "read_failed"));
    assert_eq!(TWIN, "notification_indexer_geodb_twin_total");
}

#[test]
fn pinned_ids_match_the_feeder_formula() {
    assert_eq!(membership_property("editor"), EDITOR_PROPERTY);
    assert_eq!(membership_property("member"), MEMBER_PROPERTY);
}

#[tokio::test]
async fn busy_twin_skips_instead_of_queueing() {
    let s = u(1);
    let twin = Arc::new(GeoDbTwin::new(
        Arc::new(Fake::default()),
        SpaceMap::fixed(HashMap::new()),
    ));
    // Hold the only permit: a launch must return at once without running anything.
    let _held = twin.gate.clone().try_acquire_owned().unwrap();
    twin.observe_editors(s, &[]);
    assert_eq!(twin.gate.available_permits(), 0);
}

#[test]
fn unescape_handles_tabs_newlines_and_backslashes() {
    assert_eq!(unescape(r"a\tb\nc\\d\x"), "a\tb\nc\\d\\x");
}

#[test]
fn whole_numbers_accept_zero_fractions_only() {
    assert_eq!(whole_number("1500"), Some(1500));
    assert_eq!(whole_number("1500.0"), Some(1500));
    assert_eq!(whole_number("1500.5"), None);
    assert_eq!(whole_number("x"), None);
}

#[test]
fn pinned_columns_match_the_feeders_record_kinds_when_it_is_in_the_checkout() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../geodb-feeder/src/recordKinds.ts");
    let Ok(text) = std::fs::read_to_string(path) else {
        return; // built outside the monorepo
    };
    for id in [
        VERSION_PROPOSAL,
        VERSION_BLOCK,
        VERSION_END_TIME,
        VERSION_PROPOSER,
        VERSION_SPACE,
        STATUS_PROPOSAL,
        STATUS_EXECUTED_AT,
        ESCALATION_PROPOSAL,
        VOTE_PROPOSAL,
        VOTE_VOTER,
        VERSION_NAME,
        VOTE_VERSION,
        VOTE_VOTE,
    ] {
        assert!(
            text.contains(id),
            "{id} is not in geodb-feeder/src/recordKinds.ts"
        );
    }
}

#[tokio::test]
async fn editors_granted_by_the_keyed_writer_live_in_the_governance_space() {
    let (s, a, b) = (u(1), u(2), u(3));
    let own = ent(&s);
    // a is on the own card (legacy write), b only in the governance Space (keyed write).
    let fake = Fake::default()
        .card(GEO_SPACE, &own, row(EDITOR_PROPERTY, &ent(&a), &own))
        .card(GOV, &own, row(EDITOR_PROPERTY, &ent(&b), &own));
    let sp = spaces(&s);
    assert_eq!(
        compare_editors(&fake, &sp, &s, &[a, b]).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_editors(&fake, &sp, &s, &[a]).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
    assert_eq!(
        compare_member_or_editor(&fake, &sp, &s, &b, true).await,
        vec![Verdict::Match]
    );
}

// ---- proposal name, vote tallies, latest block ----------------------------------------------

/// Version records `(record, block, name)` of the proposal in the governance Space.
fn named_versions(p: &Uuid, versions: &[(&str, i64, Option<&str>)]) -> Fake {
    let pe = ent(p);
    let mut links = String::new();
    let mut cards = String::new();
    let mut ids: Vec<String> = vec![];
    for (rec, block, name) in versions {
        links += &link(VERSION_PROPOSAL, &pe, rec);
        cards += &row(VERSION_BLOCK, &block.to_string(), rec);
        if let Some(n) = name {
            cards += &row(VERSION_NAME, n, rec);
        }
        ids.push((*rec).to_string());
    }
    ids.sort();
    Fake::default()
        .backlinks(GOV, &pe, links)
        .cards(GOV, &ids, cards)
}

async fn name_of(fake: &Fake, sql: Option<&str>) -> Vec<Verdict> {
    compare_proposal_name(fake, &spaces(&u(1)), &u(10), sql).await
}

#[tokio::test]
async fn proposal_name_follows_the_latest_version() {
    let p = u(10);
    // The older block's name is stale; only the latest block's counts.
    let fake = named_versions(&p, &[(R1, 100, Some("Old")), (R2, 200, Some("New"))]);
    assert_eq!(name_of(&fake, Some("New")).await, vec![Verdict::Match]);
    assert_eq!(
        name_of(&fake, Some("Old")).await,
        vec![Verdict::Mismatch("name_differs")]
    );
}

#[tokio::test]
async fn proposal_name_absent_cases() {
    let p = u(10);
    // No version record at all: Postgres with no row matches, Postgres with a name does not.
    let none = Fake::default();
    assert_eq!(name_of(&none, None).await, vec![Verdict::Match]);
    assert_eq!(
        name_of(&none, Some("x")).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    // A version with no name: Postgres's NULL matches, a name does not.
    let unnamed = named_versions(&p, &[(R1, 100, None)]);
    assert_eq!(name_of(&unnamed, None).await, vec![Verdict::Match]);
    assert_eq!(
        name_of(&unnamed, Some("x")).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    // A name geo-db holds that Postgres does not.
    let named = named_versions(&p, &[(R1, 100, Some("x"))]);
    assert_eq!(
        name_of(&named, None).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
}

#[tokio::test]
async fn proposal_name_unsupported_and_failed() {
    let p = u(10);
    let no_sys = Spaces::new(HashMap::new());
    assert_eq!(
        compare_proposal_name(&Fake::default(), &no_sys, &p, None).await,
        vec![Verdict::Unsupported("system_space_missing")]
    );
    // A version record with no block key cannot be ordered.
    let pe = ent(&p);
    let broken = Fake::default()
        .backlinks(GOV, &pe, link(VERSION_PROPOSAL, &pe, R1))
        .cards(GOV, &[R1.to_string()], row(VERSION_NAME, "x", R1));
    assert_eq!(
        name_of(&broken, Some("x")).await,
        vec![Verdict::Unsupported("ambiguous_record")]
    );
    assert_eq!(
        name_of(&Fake::failing(), Some("x")).await,
        vec![Verdict::Failed]
    );
}

/// Version records for blocks `1..=versions` plus one vote record per `(version, voter, vote)`.
fn tally_fixture(p: &Uuid, versions: usize, votes: &[(i64, i64)]) -> Fake {
    let pe = ent(p);
    let blocks: Vec<(String, i64)> = (0..versions)
        .map(|i| (format!("{:032x}", 0x2000 + i as u128), 100 + i as i64))
        .collect();
    let spec: Vec<(&str, i64, Option<&str>)> =
        blocks.iter().map(|(r, b)| (r.as_str(), *b, None)).collect();
    let gov = named_versions(p, &spec);
    let records: Vec<String> = (0..votes.len())
        .map(|i| format!("{:032x}", 0x3000 + i as u128))
        .collect();
    let mut links = String::new();
    let mut cards = String::new();
    for (rec, (version, vote)) in records.iter().zip(votes) {
        links += &link(VOTE_PROPOSAL, &pe, rec);
        cards += &row(VOTE_PROPOSAL, &pe, rec);
        cards += &row(VOTE_VERSION, &format!("{version}.0"), rec);
        cards += &row(VOTE_VOTE, &vote.to_string(), rec);
    }
    let mut sorted = records.clone();
    sorted.sort();
    gov.backlinks(VOTES, &pe, links)
        .cards(VOTES, &sorted, cards)
}

async fn tallies(fake: &Fake, sql: Option<(i64, i64, i64)>) -> Vec<Verdict> {
    compare_vote_tallies(fake, &spaces(&u(1)), &u(10), sql).await
}

#[tokio::test]
async fn vote_tallies_count_the_current_version_only() {
    let p = u(10);
    // Two versions: the version-1 votes are history. Current (2): yes, yes, no, abstain.
    let fake = tally_fixture(&p, 2, &[(1, 1), (1, 2), (2, 1), (2, 1), (2, 2), (2, 3)]);
    assert_eq!(tallies(&fake, Some((2, 1, 1))).await, vec![Verdict::Match]);
    assert_eq!(
        tallies(&fake, Some((3, 2, 1))).await,
        vec![Verdict::Mismatch("tally_differs")]
    );
    assert_eq!(
        tallies(&fake, Some((2, 1, 0))).await,
        vec![Verdict::Mismatch("tally_differs")]
    );
}

#[tokio::test]
async fn vote_tallies_zero_votes_match_zero() {
    let p = u(10);
    let fake = tally_fixture(&p, 1, &[]);
    assert_eq!(tallies(&fake, Some((0, 0, 0))).await, vec![Verdict::Match]);
    // After a version bump the earlier votes no longer count.
    let bumped = tally_fixture(&p, 2, &[(1, 1)]);
    assert_eq!(
        tallies(&bumped, Some((0, 0, 0))).await,
        vec![Verdict::Match]
    );
}

#[tokio::test]
async fn vote_tallies_absent_rows() {
    let p = u(10);
    // Nothing in geo-db: no Postgres row matches; a Postgres row is missing in geo-db.
    let empty = Fake::default();
    assert_eq!(tallies(&empty, None).await, vec![Verdict::Match]);
    assert_eq!(
        tallies(&empty, Some((1, 0, 0))).await,
        vec![Verdict::Mismatch("missing_in_geodb")]
    );
    // geo-db holds the proposal but Postgres has no row.
    let held = tally_fixture(&p, 1, &[(1, 1)]);
    assert_eq!(
        tallies(&held, None).await,
        vec![Verdict::Mismatch("extra_in_geodb")]
    );
}

#[tokio::test]
async fn vote_tallies_unsupported_and_failed() {
    let p = u(10);
    let no_sys = Spaces::new(HashMap::new());
    assert_eq!(
        compare_vote_tallies(&Fake::default(), &no_sys, &p, None).await,
        vec![Verdict::Unsupported("system_space_missing")]
    );
    // An unknown vote option is not guessed at.
    let odd = tally_fixture(&p, 1, &[(1, 9)]);
    assert_eq!(
        tallies(&odd, Some((0, 0, 0))).await,
        vec![Verdict::Unsupported("ambiguous_record")]
    );
    assert_eq!(
        tallies(&Fake::failing(), Some((0, 0, 0))).await,
        vec![Verdict::Failed]
    );
}

#[derive(Default)]
struct FakeCursors {
    rows: HashMap<String, Vec<u8>>,
    fail: bool,
}

#[async_trait]
impl CursorReader for FakeCursors {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TwinError> {
        if self.fail {
            return Err(TwinError::Cursors("fixture".into()));
        }
        Ok(self.rows.get(key).cloned())
    }
}

fn applied(value: &str) -> FakeCursors {
    let mut c = FakeCursors::default();
    c.rows
        .insert(APPLIED_BLOCK_KEY.to_string(), value.as_bytes().to_vec());
    c
}

#[tokio::test]
async fn latest_block_absent_row_is_unsupported_not_a_mismatch() {
    // GEODB_APPLIED_BLOCK is off: the feeder has written no row.
    assert_eq!(
        compare_latest_block(&FakeCursors::default(), Some(500)).await,
        vec![Verdict::Unsupported("row_absent")]
    );
}

#[tokio::test]
async fn latest_block_checks_the_watermark_is_not_behind() {
    let json = r#"4000:{"0":4000,"1":4100}"#;
    assert_eq!(
        compare_latest_block(&applied(json), Some(4000)).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_latest_block(&applied(json), Some(3000)).await,
        vec![Verdict::Match]
    );
    assert_eq!(
        compare_latest_block(&applied(json), Some(4001)).await,
        vec![Verdict::Mismatch("behind_sql")]
    );
}

#[tokio::test]
async fn latest_block_other_shapes() {
    assert_eq!(
        compare_latest_block(&applied("not a block"), Some(1)).await,
        vec![Verdict::Unsupported("row_unparsable")]
    );
    assert_eq!(
        compare_latest_block(&applied("10:{}"), None).await,
        vec![Verdict::Unsupported("no_sql_block")]
    );
    let failing = FakeCursors {
        fail: true,
        ..FakeCursors::default()
    };
    assert_eq!(
        compare_latest_block(&failing, Some(1)).await,
        vec![Verdict::Failed]
    );
}

#[test]
fn the_new_reads_have_their_own_kind_labels() {
    assert_eq!(
        [PROPOSAL_NAME, VOTE_TALLIES, LATEST_BLOCK],
        ["proposal_name", "vote_tallies", "latest_block"]
    );
    assert_eq!(APPLIED_BLOCK_KEY, "geodb-feeder-applied");
}

#[test]
fn applied_block_key_matches_the_feeder() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../geodb-feeder/src/appliedBlock.ts");
    let Ok(text) = std::fs::read_to_string(path) else {
        return; // built outside the monorepo
    };
    assert!(text.contains(&format!("APPLIED_BLOCK_KEY = \"{APPLIED_BLOCK_KEY}\"")));
    assert!(text.contains(&format!("CURSORS_COLLECTION = {CURSORS_COLLECTION}")));
}

// ---- the reverse of the expiry scan ------------------------------------------------------------

/// One proposal geo-db lists by end time: its gaia id, the end time of its one version record, and
/// that record's id (32 hex digits).
struct Listed {
    proposal: Uuid,
    end: i64,
    version: u128,
    executed: bool,
}

fn listed(n: u128, end: i64) -> Listed {
    Listed {
        proposal: u(n),
        end,
        version: 0x5000 + n,
        executed: false,
    }
}

fn hex(n: u128) -> String {
    format!("{n:032x}")
}

/// A fake holding a `value_page` answer per page of `pages` (each a list of rows, the page after
/// the last empty), the version cards that name each proposal, and each proposal's own records.
fn reverse_fixture(entries: &[Listed], pages: &[Vec<usize>]) -> Fake {
    let fake = Fake::default();
    let put = |space: &str, doc: String, body: String| {
        fake.answers
            .lock()
            .unwrap()
            .insert((space.into(), doc), body);
    };
    let mut after: Option<i64> = None;
    for page in pages {
        let mut body = String::new();
        let mut last = 0;
        for &i in page {
            let e = &entries[i];
            body += &format!(
                "{VERSION_END_TIME}\t{}.0\tstmt\t{}\n",
                e.end,
                hex(e.version)
            );
            last = e.end;
        }
        put(GOV, value_page_doc(FLOOR, NOW - 1, after), body);
        after = Some(last);
    }
    let mut ids: Vec<String> = entries.iter().map(|e| hex(e.version)).collect();
    ids.sort();
    ids.dedup();
    let mut naming = String::new();
    for e in entries {
        naming += &row(VERSION_PROPOSAL, &ent(&e.proposal), &hex(e.version));
    }
    for chunk in ids.chunks(MAX_CARD_IDS) {
        put(GOV, cards_doc(chunk), naming.clone());
    }
    for e in entries {
        let pe = ent(&e.proposal);
        let status = hex(0x6000 + e.version);
        let mut links = link(VERSION_PROPOSAL, &pe, &hex(e.version));
        links += &link(STATUS_PROPOSAL, &pe, &status);
        let mut cards = row(VERSION_BLOCK, "1", &hex(e.version));
        cards += &row(VERSION_END_TIME, &format!("{}.0", e.end), &hex(e.version));
        if e.executed {
            cards += &row(STATUS_EXECUTED_AT, "1900", &status);
        }
        let mut wanted = vec![hex(e.version), status];
        wanted.sort();
        put(GOV, card_doc(&pe, Some(MAX_BACKLINKS)), links);
        put(GOV, cards_doc(&wanted), cards);
    }
    fake
}

async fn reverse(fake: &Fake, sql: Option<&[Uuid]>) -> Vec<Verdict> {
    compare_expired_reverse(fake, &spaces(&u(1)), NOW, FLOOR, sql).await
}

#[tokio::test]
async fn reverse_matches_when_postgres_holds_everything_geodb_lists() {
    let e = vec![listed(10, 1100), listed(11, 1200)];
    let fake = reverse_fixture(&e, &[vec![0, 1]]);
    let sql = [u(10), u(11)];
    assert_eq!(reverse(&fake, Some(&sql)).await, vec![Verdict::Match]);
}

#[tokio::test]
async fn reverse_counts_a_proposal_only_geodb_lists() {
    let e = vec![listed(10, 1100), listed(11, 1200)];
    let fake = reverse_fixture(&e, &[vec![0, 1]]);
    assert_eq!(
        reverse(&fake, Some(&[u(10)])).await,
        vec![Verdict::Mismatch("listed_by_geodb_only")]
    );
}

#[tokio::test]
async fn reverse_resumes_through_a_tie_and_across_pages() {
    // Two proposals tie at 1100 (one page, never split); the next page holds 1300.
    let e = vec![listed(10, 1100), listed(11, 1100), listed(12, 1300)];
    let fake = reverse_fixture(&e, &[vec![0, 1], vec![2]]);
    assert_eq!(
        reverse(&fake, Some(&[])).await,
        vec![Verdict::Mismatch("listed_by_geodb_only"); 3]
    );
    // Held by Postgres: only the proposal on the resumed page is missing.
    assert_eq!(
        reverse(&fake, Some(&[u(10), u(11)])).await,
        vec![Verdict::Mismatch("listed_by_geodb_only")]
    );
}

#[tokio::test]
async fn reverse_empty_page_is_a_match() {
    let fake = Fake::default();
    assert_eq!(reverse(&fake, Some(&[])).await, vec![Verdict::Match]);
}

#[tokio::test]
async fn reverse_ignores_an_executed_proposal() {
    let mut x = listed(10, 1100);
    x.executed = true;
    let fake = reverse_fixture(&[x], &[vec![0]]);
    assert_eq!(
        reverse(&fake, Some(&[])).await,
        vec![Verdict::Unsupported("superseded_or_executed")]
    );
}

#[tokio::test]
async fn reverse_refusal_and_unreadable_sql_side() {
    // An old node refuses the read: failed, never a mismatch.
    assert_eq!(
        reverse(&Fake::failing(), Some(&[])).await,
        vec![Verdict::Failed]
    );
    assert_eq!(
        reverse(&Fake::default(), None).await,
        vec![Verdict::Unsupported("sql_window_too_large")]
    );
    let no_gov = Spaces::new(HashMap::new());
    assert_eq!(
        compare_expired_reverse(&Fake::default(), &no_gov, NOW, FLOOR, Some(&[])).await,
        vec![Verdict::Unsupported("system_space_missing")]
    );
}

#[test]
fn reverse_has_its_own_kind_label_and_the_value_page_call_text() {
    assert_eq!(EXPIRED_PROPOSALS_REVERSE, "expired_proposals_reverse");
    assert_eq!(
        value_page_doc(10, 20, Some(15)),
        format!(
            ".{{ .version = 1, .select = .{{ .c = .{{ .field = \"entity\", .args = .{{ \
             .value_page = 512, .value_property = \"{VERSION_END_TIME}\", .value_from = 10, \
             .value_to = 20, .value_after = 15 }} }} }} }}"
        )
    );
}
