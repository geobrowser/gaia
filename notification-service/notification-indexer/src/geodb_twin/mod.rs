//! A geo-db twin of the notification-indexer's Postgres reads of the chain tables, behind a flag
//! that defaults off.
//!
//! With `NOTIFICATION_INDEXER_GEODB_TWIN=1`, after a SQL read succeeds the same question is asked
//! of geo-db's governance records on a background task, and the two answers are compared. Postgres
//! stays the source of every answer: the twin never changes what a read returns, never delays it,
//! and never fails it. What leaves the process is a count, [`TWIN`] (`read`, `result`, `why`), and a
//! `warn!` carrying counts only (no ids) for each disagreement, which is the evidence for moving
//! each read off Postgres.
//!
//! The comparison runs in geo-db's id space (see [`ids`]): Postgres ids are conformed forward the
//! way the feeder conforms them, and gaia Spaces are mapped through the feeder's space map. A Space
//! the map does not hold is skipped as `unsupported`, never reported as a gap.
//!
//! Twinned, each over records the feeder writes today:
//! - `editors` (`find_editors_for_space`): the editor edges on the Space's own entity card in the
//!   Space's own geo-db Space.
//! - `member_or_editor` (`is_member_or_editor`): the same card, editor and member edges.
//! - `voters` (`find_voters_for_proposal`): the proposal's vote records in the `votes` system Space,
//!   found by backlinks to the proposal entity, each read for its voter.
//! - `proposer`, `proposer_and_space` (existence only): whether the proposal has a version record
//!   in the `governance` system Space.
//! - `expired_proposals` (`find_expired_proposals`): for each row Postgres returned, whether the
//!   proposal's latest version record and status record also say "ended, not executed".
//!
//! Left on Postgres only, each for a reason:
//! - the proposer (`proposed_by`) and owning Space (`proposals.space_id`) values: no record holds
//!   them (a version record carries settings and times, a vote record the voting Space but not the
//!   proposal's owner), so only existence can be twinned.
//! - the reverse of the expiry scan (a proposal geo-db says expired that Postgres did not list):
//!   geo-db has no read that lists proposals, so it cannot be enumerated; the twin only judges rows
//!   Postgres listed. The outbox anti-join half of the query is a table geo-db mirrors elsewhere.
//! - a proposal with a settings-escalation record: the end time in force depends on the escalation,
//!   so it is `unsupported` (`escalation_pending`).
//! - `lookup_vote_tallies` and `lookup_proposal_name` (`proposals_current` columns): derived per
//!   version, no twin built yet.
//! - `lookup_latest_block` (`proposals.created_at_block`): not carried by a record.
//! - the relation, value and `votes_count` reads (`lookup_entity_space`, `lookup_bounty_space`,
//!   thread reads, `lookup_entity_name`, `lookup_space_name`, `find_entity_vote_counts_since`):
//!   not chain-table reads; out of scope here.

pub mod ids;
pub mod pipe;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hermes_instrumentation::{info, warn};
use tokio::sync::Semaphore;
use uuid::Uuid;

use ids::{conforming_compact, conforming_id, is_hex32, SpaceMap, Spaces};
use pipe::{PipeConfig, PipeReader};

/// The flag: `1` or `true` turns the twin on. Anything else, or unset, leaves it off.
pub const FLAG: &str = "NOTIFICATION_INDEXER_GEODB_TWIN";
pub const TIMEOUT_VAR: &str = "NOTIFICATION_INDEXER_GEODB_TWIN_TIMEOUT_MS";

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5000);
const COOLDOWN: Duration = Duration::from_secs(30);
/// Most entities one batch card read names (geo-db's bound).
const MAX_CARD_IDS: usize = 32;
/// Most backlinks one read asks for (geo-db's `MAX_TOPK_LIMIT`); that many back means there may be more.
const MAX_BACKLINKS: usize = 4096;

/// The one counter family: `read`, `result` = `match` | `mismatch` | `unsupported` | `failed`, and
/// `why` (`none` for a match; the kind of disagreement; the reason a comparison could not be made).
pub const TWIN: &str = "notification_indexer_geodb_twin_total";
/// Comparisons not started: `read`, `reason` = `busy` | `space_map_unreadable`. A skip is not a result.
pub const TWIN_SKIPPED: &str = "notification_indexer_geodb_twin_skipped_total";

pub const EDITORS: &str = "editors";
pub const MEMBER_OR_EDITOR: &str = "member_or_editor";
pub const VOTERS: &str = "voters";
pub const PROPOSER: &str = "proposer";
pub const PROPOSER_AND_SPACE: &str = "proposer_and_space";
pub const EXPIRED_PROPOSALS: &str = "expired_proposals";

/// The pinned ids the twin reads, copied from `geodb-feeder` (`membership.ts`, `recordKinds.ts`).
/// A test fails if the feeder's file is in the checkout and any copy differs.
pub mod pinned {
    /// `ROLE_PROPERTY.editor` and `.member`: `conformingId("membership-property", role)`.
    pub const EDITOR_PROPERTY: &str = "79c3a498486641bc930f3b973de4b4d3";
    pub const MEMBER_PROPERTY: &str = "dbe3bdf637a5458d9206d737fb104793";
    /// `RECORD_COLUMNS.proposalVersion`: `proposal` (key reference), `block` (key), `endTime`.
    pub const VERSION_PROPOSAL: &str = "c42be9e340d14566b2b9f084004c748b";
    pub const VERSION_BLOCK: &str = "bccb84d7017849b0babfa7444789068f";
    pub const VERSION_END_TIME: &str = "72d87497d7ed49db81e31675a31c5da7";
    /// `proposalStatus`: `proposal` (key reference), `executedAt`.
    pub const STATUS_PROPOSAL: &str = "4f742a5f821a469c9c0be0b352ae5b67";
    pub const STATUS_EXECUTED_AT: &str = "ed3855a3c41b4f749651bf049dabac0b";
    /// `proposalEscalation`: `proposal` (key reference).
    pub const ESCALATION_PROPOSAL: &str = "49c19599b0694221841396f0db5f9995";
    /// `proposalVote`: `proposal` and `voter` (key references).
    pub const VOTE_PROPOSAL: &str = "c020ebd69bbb4bec8c83db30765aa91b";
    pub const VOTE_VOTER: &str = "e033f679eebb40aeaec8c66f9a1d6af9";
}
use pinned::*;

/// Describe the twin's series. Call once, after the recorder is installed.
pub fn describe() {
    metrics::describe_counter!(
        TWIN,
        "Notification-indexer Postgres reads also answered from geo-db records, by read, result \
         (match, mismatch, unsupported, failed) and why"
    );
    metrics::describe_counter!(
        TWIN_SKIPPED,
        "Twin comparisons not started, by read and reason (busy, space_map_unreadable)"
    );
}

#[derive(Debug, thiserror::Error)]
pub enum TwinError {
    #[error("geodb-client did not start: {0}")]
    Spawn(std::io::Error),
    #[error("geodb-client i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("geodb-client closed its output")]
    Closed,
    #[error("geodb-client did not say ready: {0:?}")]
    NotReady(String),
    #[error("geodb-client sent a bad frame header: {0:?}")]
    BadFrame(String),
    #[error("geo-db refused the read: {0}")]
    Refused(String),
    #[error("geo-db read timed out after {0:?}")]
    Timeout(Duration),
    #[error("geodb-client failed recently; cooling down")]
    CoolingDown,
    #[error("request has no Spaces or breaks the line format")]
    BadRequest,
    #[error("space map: {0}")]
    SpaceMap(String),
}

/// One geo-db read: a GeoQL document over a scope of geo-db Spaces, answering rows (one a line,
/// tab-separated cells) or a refusal.
#[allow(clippy::double_must_use)] // newer clippy flags async_trait's own must_use
#[async_trait]
pub trait GeoDbReader: Send + Sync {
    async fn query(&self, spaces: &[String], doc: &str) -> Result<String, TwinError>;
}

#[derive(Clone, Debug)]
pub struct TwinConfig {
    pub pipe: PipeConfig,
    pub space_map: PathBuf,
}

impl TwinConfig {
    /// `Ok(None)` when the flag is off. With it on, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH` and
    /// `GEODB_SPACE_MAP_PATH` are required; `GEODB_CLIENT_BIN` defaults to `geodb-client` on `PATH`.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let on = get(FLAG).is_some_and(|v| matches!(v.trim(), "1" | "true"));
        if !on {
            return Ok(None);
        }
        let need = |name: &str| {
            get(name)
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{FLAG} is on but {name} is not set"))
        };
        let key = need("GEODB_KEY_PATH")?;
        let binding = need("GEODB_BINDING_PATH")?;
        let space_map = need("GEODB_SPACE_MAP_PATH")?;
        let program = get("GEODB_CLIENT_BIN")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "geodb-client".into());
        let timeout = match get(TIMEOUT_VAR) {
            None => DEFAULT_TIMEOUT,
            Some(v) => v
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis)
                .ok_or_else(|| format!("{TIMEOUT_VAR} must be a positive number of ms"))?,
        };
        Ok(Some(Self {
            pipe: PipeConfig {
                program: program.into(),
                args: vec![
                    "--key".into(),
                    key.into(),
                    "--binding".into(),
                    binding.into(),
                    "pipe".into(),
                ],
                timeout,
                cooldown: COOLDOWN,
            },
            space_map: space_map.into(),
        }))
    }
}

/// What one compared item found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Match,
    /// The two stores disagree; the string says how.
    Mismatch(&'static str),
    /// The records cannot decide this item; the string says why.
    Unsupported(&'static str),
    /// A geo-db read failed.
    Failed,
}

impl Verdict {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Verdict::Match => ("match", "none"),
            Verdict::Mismatch(w) => ("mismatch", w),
            Verdict::Unsupported(w) => ("unsupported", w),
            Verdict::Failed => ("failed", "read_failed"),
        }
    }
}

/// Count verdicts, and log a count line (no ids) for each kind that is not a match.
pub fn record(read: &'static str, verdicts: &[Verdict]) {
    let mut counts: HashMap<(&'static str, &'static str), u64> = HashMap::new();
    for v in verdicts {
        *counts.entry(v.labels()).or_default() += 1;
    }
    for ((result, why), n) in counts {
        metrics::counter!(TWIN, "read" => read, "result" => result, "why" => why).increment(n);
        if result != "match" {
            warn!(
                event = "geodb_twin_summary",
                read = read,
                result = result,
                why = why,
                count = n,
                "geo-db twin did not agree with Postgres"
            );
        }
    }
}

fn skipped(read: &'static str, reason: &'static str) {
    metrics::counter!(TWIN_SKIPPED, "read" => read, "reason" => reason).increment(1);
}

/// A card row: a property and the value geo-db printed for it.
type CardRow = (String, String);

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The cells of each row line; blank and `#` (plan, comment) lines are skipped.
fn rows(body: &str) -> impl Iterator<Item = Vec<&str>> {
    body.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').collect())
}

fn card_doc(entity: &str, backlinks: Option<usize>) -> String {
    let backlinks = backlinks.map_or(String::new(), |n| format!(", .backlinks = {n}"));
    format!(
        ".{{ .version = 1, .select = .{{ .c = .{{ .field = \"entity\", \
         .args = .{{ .id = \"{entity}\"{backlinks} }} }} }} }}"
    )
}

fn cards_doc(ids: &[String]) -> String {
    let ids = ids
        .iter()
        .map(|i| format!("\"{i}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        ".{{ .version = 1, .select = .{{ .c = .{{ .field = \"entity\", \
         .args = .{{ .ids = .{{ {ids} }} }} }} }} }}"
    )
}

/// The card rows of one entity in one Space.
async fn read_card(
    reader: &dyn GeoDbReader,
    space: &str,
    entity: &str,
) -> Result<Vec<CardRow>, TwinError> {
    let body = reader
        .query(&[space.to_string()], &card_doc(entity, None))
        .await?;
    Ok(rows(&body)
        .filter(|c| c.get(3) == Some(&entity))
        .map(|c| {
            (
                c.first().copied().unwrap_or("").to_string(),
                unescape(c.get(1).copied().unwrap_or("")),
            )
        })
        .collect())
}

/// The card rows of any number of entities in one Space, keyed by owning entity (batched by 32).
async fn read_cards(
    reader: &dyn GeoDbReader,
    space: &str,
    ids: &[String],
) -> Result<HashMap<String, Vec<CardRow>>, TwinError> {
    let mut by: HashMap<String, Vec<CardRow>> = HashMap::new();
    for chunk in ids.chunks(MAX_CARD_IDS) {
        let body = reader
            .query(&[space.to_string()], &cards_doc(chunk))
            .await?;
        for c in rows(&body) {
            let Some(owner) = c.get(3).filter(|o| is_hex32(o)) else {
                continue;
            };
            by.entry((*owner).to_string()).or_default().push((
                c.first().copied().unwrap_or("").to_string(),
                unescape(c.get(1).copied().unwrap_or("")),
            ));
        }
    }
    Ok(by)
}

/// The statements referring to `entity` in one Space: `(property, referring entity)`.
async fn read_backlinks(
    reader: &dyn GeoDbReader,
    space: &str,
    entity: &str,
) -> Result<Vec<(String, String)>, TwinError> {
    let body = reader
        .query(&[space.to_string()], &card_doc(entity, Some(MAX_BACKLINKS)))
        .await?;
    Ok(rows(&body)
        .filter(|c| c.get(1) == Some(&entity) && c.get(3).is_some_and(|f| is_hex32(f)))
        .map(|c| {
            (
                c.first().copied().unwrap_or("").to_string(),
                c[3].to_string(),
            )
        })
        .collect())
}

/// The entity a gaia id has in geo-db.
fn entity_ref(id: &Uuid) -> String {
    conforming_id("entity", id)
}

/// The referenced entities held under `property` on a card.
fn refs(card: &[CardRow], property: &str) -> BTreeSet<String> {
    card.iter()
        .filter(|(p, v)| p == property && is_hex32(v))
        .map(|(_, v)| v.clone())
        .collect()
}

fn set_verdict(expected: &BTreeSet<String>, got: &BTreeSet<String>) -> Verdict {
    let missing = expected.difference(got).count();
    let extra = got.difference(expected).count();
    match (missing, extra) {
        (0, 0) => Verdict::Match,
        (_, 0) => Verdict::Mismatch("missing_in_geodb"),
        (0, _) => Verdict::Mismatch("extra_in_geodb"),
        _ => Verdict::Mismatch("differs"),
    }
}

/// The geo-db Space holding a gaia Space's own entity card, or the verdict to give when it has none.
fn own_space<'a>(spaces: &'a Spaces, space: &Uuid) -> Result<&'a str, Verdict> {
    spaces
        .geo_of(space)
        .ok_or(Verdict::Unsupported("space_not_loaded"))
}

/// Twin of `find_editors_for_space`: the editor edges on the Space's own entity card, as a set of
/// member entities, against the editors Postgres returned.
pub async fn compare_editors(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    space: &Uuid,
    sql_editors: &[Uuid],
) -> Vec<Verdict> {
    let geo = match own_space(spaces, space) {
        Ok(g) => g,
        Err(v) => return vec![v],
    };
    let expected: BTreeSet<String> = sql_editors.iter().map(entity_ref).collect();
    match read_card(reader, geo, &entity_ref(space)).await {
        Ok(card) => vec![set_verdict(&expected, &refs(&card, EDITOR_PROPERTY))],
        Err(_) => vec![Verdict::Failed],
    }
}

/// Twin of `is_member_or_editor`: whether the user holds an editor or a member edge on the Space's
/// own entity card, against the boolean Postgres returned.
pub async fn compare_member_or_editor(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    space: &Uuid,
    user: &Uuid,
    sql_answer: bool,
) -> Vec<Verdict> {
    let geo = match own_space(spaces, space) {
        Ok(g) => g,
        Err(v) => return vec![v],
    };
    match read_card(reader, geo, &entity_ref(space)).await {
        Ok(card) => {
            let user = entity_ref(user);
            let held = refs(&card, EDITOR_PROPERTY).contains(&user)
                || refs(&card, MEMBER_PROPERTY).contains(&user);
            vec![match (sql_answer, held) {
                (true, false) => Verdict::Mismatch("missing_in_geodb"),
                (false, true) => Verdict::Mismatch("extra_in_geodb"),
                _ => Verdict::Match,
            }]
        }
        Err(_) => vec![Verdict::Failed],
    }
}

/// Twin of `find_voters_for_proposal`: the distinct voters named by the proposal's vote records in
/// the `votes` system Space, against the voters Postgres returned.
pub async fn compare_voters(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    proposal: &Uuid,
    sql_voters: &[Uuid],
) -> Vec<Verdict> {
    let Some(votes) = spaces.system("votes") else {
        return vec![Verdict::Unsupported("system_space_missing")];
    };
    let result: Result<Verdict, TwinError> = async {
        let links = read_backlinks(reader, votes, &entity_ref(proposal)).await?;
        if links.len() >= MAX_BACKLINKS {
            return Ok(Verdict::Unsupported("too_many_links"));
        }
        let records: Vec<String> = links
            .into_iter()
            .filter(|(p, _)| p == VOTE_PROPOSAL)
            .map(|(_, from)| from)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let cards = read_cards(reader, votes, &records).await?;
        let mut got = BTreeSet::new();
        for id in &records {
            let voters = refs(cards.get(id).map_or(&[][..], Vec::as_slice), VOTE_VOTER);
            if voters.len() > 1 {
                return Ok(Verdict::Unsupported("ambiguous_record"));
            }
            got.extend(voters);
        }
        let expected: BTreeSet<String> = sql_voters.iter().map(entity_ref).collect();
        Ok(set_verdict(&expected, &got))
    }
    .await;
    vec![result.unwrap_or(Verdict::Failed)]
}

/// Whether the proposal has at least one version record in the `governance` system Space.
async fn proposal_has_version(
    reader: &dyn GeoDbReader,
    governance: &str,
    proposal: &Uuid,
) -> Result<bool, TwinError> {
    let links = read_backlinks(reader, governance, &entity_ref(proposal)).await?;
    Ok(links.iter().any(|(p, _)| p == VERSION_PROPOSAL))
}

/// Twin of `find_proposer_for_proposal` and `find_proposal_proposer_and_space`, existence only:
/// Postgres answered a row (`Some`) or none (`None`); geo-db says whether the proposal has a
/// version record. The proposer and Space values are not in any record (see the module doc).
pub async fn compare_proposal_exists(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    proposal: &Uuid,
    sql_exists: bool,
) -> Vec<Verdict> {
    let Some(governance) = spaces.system("governance") else {
        return vec![Verdict::Unsupported("system_space_missing")];
    };
    vec![
        match proposal_has_version(reader, governance, proposal).await {
            Ok(held) => match (sql_exists, held) {
                (true, false) => Verdict::Mismatch("missing_in_geodb"),
                (false, true) => Verdict::Mismatch("extra_in_geodb"),
                _ => Verdict::Match,
            },
            Err(_) => Verdict::Failed,
        },
    ]
}

/// A whole number as geo-db prints it (`1`, or `1.0`).
fn whole_number(raw: &str) -> Option<i64> {
    let (int, frac) = raw.split_once('.').unwrap_or((raw, ""));
    if !frac.bytes().all(|b| b == b'0') {
        return None;
    }
    int.parse().ok()
}

/// The one value under `property`, `Err` when the card holds two different ones.
fn single<'a>(card: &'a [CardRow], property: &str) -> Result<Option<&'a str>, ()> {
    let values: BTreeSet<&str> = card
        .iter()
        .filter(|(p, _)| p == property)
        .map(|(_, v)| v.as_str())
        .collect();
    match values.len() {
        0 => Ok(None),
        1 => Ok(values.into_iter().next()),
        _ => Err(()),
    }
}

/// Judge one row of the expiry scan from the proposal's records. `now` and `floor` are epoch
/// seconds; `sql_end_time` is the row's `end_time`.
async fn judge_expired(
    reader: &dyn GeoDbReader,
    governance: &str,
    proposal: &Uuid,
    sql_end_time: i64,
    now: i64,
    floor: i64,
) -> Result<Verdict, TwinError> {
    let links = read_backlinks(reader, governance, &entity_ref(proposal)).await?;
    if links.len() >= MAX_BACKLINKS {
        return Ok(Verdict::Unsupported("too_many_links"));
    }
    let from = |prop: &str| -> Vec<String> {
        links
            .iter()
            .filter(|(p, _)| p == prop)
            .map(|(_, f)| f.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    if !from(ESCALATION_PROPOSAL).is_empty() {
        return Ok(Verdict::Unsupported("escalation_pending"));
    }
    let versions = from(VERSION_PROPOSAL);
    let statuses = from(STATUS_PROPOSAL);
    let wanted: Vec<String> = versions.iter().chain(statuses.iter()).cloned().collect();
    let cards = read_cards(reader, governance, &wanted).await?;
    let card = |id: &String| cards.get(id).map_or(&[][..], Vec::as_slice);

    // The latest version (highest block) carries the end time in force.
    let mut latest: Option<(i64, i64)> = None;
    for id in &versions {
        let (Ok(block), Ok(end)) = (
            single(card(id), VERSION_BLOCK),
            single(card(id), VERSION_END_TIME),
        ) else {
            return Ok(Verdict::Unsupported("ambiguous_record"));
        };
        let (Some(block), Some(end)) = (block.and_then(whole_number), end.and_then(whole_number))
        else {
            return Ok(Verdict::Unsupported("ambiguous_record"));
        };
        if latest.is_none_or(|(b, _)| block > b) {
            latest = Some((block, end));
        }
    }
    let Some((_, end_time)) = latest else {
        return Ok(Verdict::Mismatch("no_version_record"));
    };
    for id in &statuses {
        match single(card(id), STATUS_EXECUTED_AT) {
            Ok(None) => {}
            Ok(Some(_)) => return Ok(Verdict::Mismatch("executed_in_geodb")),
            Err(()) => return Ok(Verdict::Unsupported("ambiguous_record")),
        }
    }
    if end_time != sql_end_time {
        return Ok(Verdict::Mismatch("end_time_differs"));
    }
    if end_time >= now || end_time < floor {
        return Ok(Verdict::Mismatch("not_expired_in_geodb"));
    }
    Ok(Verdict::Match)
}

/// Twin of `find_expired_proposals`: for each `(proposal, end_time)` row Postgres returned, whether
/// the proposal's records also say it ended inside the window and was not executed. Only the rows
/// Postgres listed are judged (geo-db cannot list proposals).
pub async fn compare_expired_proposals(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    now: i64,
    floor: i64,
    sql_rows: &[(Uuid, i64)],
) -> Vec<Verdict> {
    let Some(governance) = spaces.system("governance") else {
        return sql_rows
            .iter()
            .map(|_| Verdict::Unsupported("system_space_missing"))
            .collect();
    };
    let mut out = Vec::with_capacity(sql_rows.len());
    for (proposal, end) in sql_rows {
        match judge_expired(reader, governance, proposal, *end, now, floor).await {
            Ok(v) => out.push(v),
            Err(_) => {
                // One failed read ends the run: the pipe is cooling down.
                out.push(Verdict::Failed);
                break;
            }
        }
    }
    out
}

/// The twin a [`crate::storage::Storage`] holds. Comparisons run one at a time on a background
/// task; a read that finds one already running counts `busy` and moves on, so the twin's cost is
/// bounded by one in-flight comparison whatever the indexer's rate.
pub struct GeoDbTwin {
    reader: Arc<dyn GeoDbReader>,
    spaces: SpaceMap,
    gate: Arc<Semaphore>,
}

/// Most rows of one expiry scan the twin judges (the scan returns up to its batch limit).
const EXPIRED_SAMPLE: usize = 8;

impl GeoDbTwin {
    pub fn new(reader: Arc<dyn GeoDbReader>, spaces: SpaceMap) -> Self {
        Self {
            reader,
            spaces,
            gate: Arc::new(Semaphore::new(1)),
        }
    }

    /// From the environment: `None` when the flag is off, or when it is on but misconfigured
    /// (logged loudly, and the indexer carries on without it: the twin must never stop the indexer).
    pub fn from_env() -> Option<Arc<Self>> {
        match TwinConfig::from_vars(|k| std::env::var(k).ok()) {
            Ok(None) => None,
            Ok(Some(cfg)) => {
                info!(
                    timeout_ms = cfg.pipe.timeout.as_millis() as u64,
                    "geo-db twin on: chain-table reads are compared with geo-db records"
                );
                describe();
                Some(Arc::new(Self::new(
                    Arc::new(PipeReader::new(cfg.pipe)),
                    SpaceMap::from_path(cfg.space_map),
                )))
            }
            Err(e) => {
                warn!(error = %e, "geo-db twin is misconfigured; running without it");
                None
            }
        }
    }

    /// Run `compare` on a background task if no other comparison is in flight. Never blocks, never
    /// fails the caller; outside a runtime it does nothing.
    fn launch<F, Fut>(self: &Arc<Self>, read: &'static str, compare: F)
    where
        F: FnOnce(Arc<dyn GeoDbReader>, Arc<Spaces>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Vec<Verdict>> + Send + 'static,
    {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(permit) = self.gate.clone().try_acquire_owned() else {
            skipped(read, "busy");
            return;
        };
        let spaces = match self.spaces.current() {
            Ok(s) => s,
            Err(e) => {
                skipped(read, "space_map_unreadable");
                warn!(error = %e, read = read, "geo-db twin skipped: no space map");
                return;
            }
        };
        let reader = self.reader.clone();
        rt.spawn(async move {
            // No outer timeout: each read is bounded inside the reader, and cancelling a read
            // half-way would leave its reply in the pipe for the next one.
            let verdicts = compare(reader, spaces).await;
            record(read, &verdicts);
            drop(permit);
        });
    }

    /// After `find_editors_for_space(space)` answered `editors`.
    pub fn observe_editors(self: &Arc<Self>, space: Uuid, editors: &[Uuid]) {
        let editors = editors.to_vec();
        self.launch(EDITORS, move |r, s| async move {
            compare_editors(r.as_ref(), &s, &space, &editors).await
        });
    }

    /// After `is_member_or_editor(space, user)` answered `answer`.
    pub fn observe_member_or_editor(self: &Arc<Self>, space: Uuid, user: Uuid, answer: bool) {
        self.launch(MEMBER_OR_EDITOR, move |r, s| async move {
            compare_member_or_editor(r.as_ref(), &s, &space, &user, answer).await
        });
    }

    /// After `find_voters_for_proposal(proposal)` answered `voters`.
    pub fn observe_voters(self: &Arc<Self>, proposal: Uuid, voters: &[Uuid]) {
        let voters = voters.to_vec();
        self.launch(VOTERS, move |r, s| async move {
            compare_voters(r.as_ref(), &s, &proposal, &voters).await
        });
    }

    /// After a proposer lookup (`read` is [`PROPOSER`] or [`PROPOSER_AND_SPACE`]) found a row or not.
    pub fn observe_proposal_exists(
        self: &Arc<Self>,
        read: &'static str,
        proposal: Uuid,
        exists: bool,
    ) {
        self.launch(read, move |r, s| async move {
            compare_proposal_exists(r.as_ref(), &s, &proposal, exists).await
        });
    }

    /// After `find_expired_proposals` answered `rows` (`(proposal, end_time)`).
    pub fn observe_expired_proposals(self: &Arc<Self>, floor: i64, rows: &[(Uuid, i64)]) {
        if rows.is_empty() {
            return;
        }
        let rows: Vec<(Uuid, i64)> = rows.iter().take(EXPIRED_SAMPLE).copied().collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        self.launch(EXPIRED_PROPOSALS, move |r, s| async move {
            compare_expired_proposals(r.as_ref(), &s, now, floor, &rows).await
        });
    }
}

/// The geo-db id of a pinned membership property name, as the feeder mints it.
pub fn membership_property(role: &str) -> String {
    conforming_compact("membership-property", role)
}

#[cfg(test)]
mod tests;
