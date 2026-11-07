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
        STATUS_PROPOSAL,
        STATUS_EXECUTED_AT,
        ESCALATION_PROPOSAL,
        VOTE_PROPOSAL,
        VOTE_VOTER,
    ] {
        assert!(
            text.contains(id),
            "{id} is not in geodb-feeder/src/recordKinds.ts"
        );
    }
}
