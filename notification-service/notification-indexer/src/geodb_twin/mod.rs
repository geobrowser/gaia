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
//! - `editors` (`find_editors_for_space`): the editor edges on the Space's own entity card, in the
//!   Space's own geo-db Space and in the `governance` system Space (where the keyed writer puts them).
//! - `member_or_editor` (`is_member_or_editor`): the same card, editor and member edges.
//! - `voters` (`find_voters_for_proposal`): the proposal's vote records in the `votes` system Space,
//!   found by backlinks to the proposal entity, each read for its voter.
//! - `proposer`, `proposer_and_space` (`find_proposer_for_proposal`, `find_proposal_proposer_and_space`):
//!   whether the proposal has a version record in the `governance` system Space and, when Postgres
//!   returned a row, whether the version records' `proposer` (and `space`) references name the same
//!   proposer (and owning Space). Those references are written only when the feeder runs with
//!   `GEODB_PROPOSAL_VERSION_PROPOSER_SPACE`; a proposal whose version records hold none is
//!   `unsupported` (`values_not_stored`), never a mismatch.
//! - `expired_proposals` (`find_expired_proposals`): for each row Postgres returned, whether the
//!   proposal's latest version record and status record also say "ended, not executed".
//!
//! Left on Postgres only, each for a reason:
//! - the reverse of the expiry scan (a proposal geo-db says expired that Postgres did not list):
//!   geo-db has no read that lists proposals, so it cannot be enumerated; the twin only judges rows
//!   Postgres listed. The outbox anti-join half of the query is a table geo-db mirrors elsewhere.
//! - a proposal with a settings-escalation record: the end time in force depends on the escalation,
//!   so it is `unsupported` (`escalation_pending`).
//! - `proposal_name` (`lookup_proposal_name`, `proposals_current.name`): the `name` of the proposal's
//!   latest version record (highest `block`). A name Postgres holds that the record lacks is
//!   `missing_in_geodb`; one the record holds that Postgres lacks is `extra_in_geodb`.
//! - `vote_tallies` (`lookup_vote_tallies`, `proposals_current` yes/no/abstain counts): counted from the
//!   proposal's `proposalVote` records in the `votes` system Space, over the current version only. The
//!   current version is the number of version records (kg-indexer numbers versions 1..N as it inserts
//!   them), the same rule as its tally worker's `current_version` filter. Postgres's counts come from a
//!   worker that runs after the votes land, so a `tally_differs` right after a vote can be lag.
//! - `latest_block` (`lookup_latest_block`, `MAX(proposals.created_at_block)`): geo-db holds no
//!   per-proposal block on a record, only the feeder's applied-block watermark in `cursors` (key
//!   `geodb-feeder-applied`). They are different quantities, so the one claim checked is that the
//!   watermark is not behind Postgres's block (`behind_sql`). An absent row (the feeder's
//!   `GEODB_APPLIED_BLOCK` is off) or an unparsable one is `unsupported`, never a mismatch.
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
const COOLDOWN: Duration = Duration::from_secs(5);
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
pub const PROPOSAL_NAME: &str = "proposal_name";
pub const VOTE_TALLIES: &str = "vote_tallies";
pub const LATEST_BLOCK: &str = "latest_block";

/// The feeder's applied-block row: `cursors` key and collection (`geodb-feeder/src/appliedBlock.ts`).
pub const APPLIED_BLOCK_KEY: &str = "geodb-feeder-applied";
pub const CURSORS_COLLECTION: u32 = 10;

/// The pinned ids the twin reads, copied from `geodb-feeder` (`membership.ts`, `recordKinds.ts`).
/// A test fails if the feeder's file is in the checkout and any copy differs.
pub mod pinned {
    /// `ROLE_PROPERTY.editor` and `.member`: `conformingId("membership-property", role)`.
    pub const EDITOR_PROPERTY: &str = "79c3a498486641bc930f3b973de4b4d3";
    pub const MEMBER_PROPERTY: &str = "dbe3bdf637a5458d9206d737fb104793";
    /// `RECORD_COLUMNS.proposalVersion`: `proposal` (key reference), `block` (key), `endTime`, and the
    /// flag-gated `proposer` and `space` references.
    pub const VERSION_PROPOSAL: &str = "c42be9e340d14566b2b9f084004c748b";
    pub const VERSION_BLOCK: &str = "bccb84d7017849b0babfa7444789068f";
    pub const VERSION_END_TIME: &str = "72d87497d7ed49db81e31675a31c5da7";
    pub const VERSION_PROPOSER: &str = "e95e27260a364e95877b3973746a9ca6";
    pub const VERSION_SPACE: &str = "e35197b9528d4b04a7e8583d0c20a5b9";
    /// `proposalVersion.name`: text, present when the version has actions.
    pub const VERSION_NAME: &str = "a76bbfa24cfa43708b89326ae5f72e18";
    /// `proposalStatus`: `proposal` (key reference), `executedAt`.
    pub const STATUS_PROPOSAL: &str = "4f742a5f821a469c9c0be0b352ae5b67";
    pub const STATUS_EXECUTED_AT: &str = "ed3855a3c41b4f749651bf049dabac0b";
    /// `proposalEscalation`: `proposal` (key reference).
    pub const ESCALATION_PROPOSAL: &str = "49c19599b0694221841396f0db5f9995";
    /// `proposalVote`: `proposal` and `voter` (key references).
    pub const VOTE_PROPOSAL: &str = "c020ebd69bbb4bec8c83db30765aa91b";
    pub const VOTE_VOTER: &str = "e033f679eebb40aeaec8c66f9a1d6af9";
    /// `proposalVote.version` (key, number) and `.vote` (number: 1 yes, 2 no, 3 abstain).
    pub const VOTE_VERSION: &str = "da1d8b5c47ee499797e729ae90938086";
    pub const VOTE_VOTE: &str = "328dc69e2c964982a35fe4d3aa79e31a";
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
    #[error("geo-db cursors read: {0}")]
    Cursors(String),
}

/// One read of a row of geo-db's `cursors` collection (the derived class), by key.
#[async_trait]
pub trait CursorReader: Send + Sync {
    /// The value stored under `key`, `None` when the collection holds no such row.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TwinError>;
}

/// [`CursorReader`] over the held `geodb-client derived pipe` child.
pub struct DerivedCursors {
    reader: geodb_cursor::derived_read::DerivedReader,
    collection: u32,
}

impl DerivedCursors {
    pub fn new(reader: geodb_cursor::derived_read::DerivedReader, collection: u32) -> Self {
        Self { reader, collection }
    }

    /// Built from the same flag and paths as the twin; `None` when the flag is off.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        geodb_cursor::derived_read::DerivedReader::from_vars_for(FLAG, get)
            .map(|r| Self::new(r, CURSORS_COLLECTION))
    }
}

#[async_trait]
impl CursorReader for DerivedCursors {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TwinError> {
        let got = self
            .reader
            .get_many(self.collection, &[key.as_bytes().to_vec()])
            .await
            .map_err(|e| TwinError::Cursors(e.to_string()))?;
        Ok(got.into_iter().next().flatten().map(|g| g.value))
    }
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

/// The Space's membership edges, wherever the feeder put them: the Space's own geo-db Space (the
/// legacy dual write) and the `governance` system Space (the natural-key writer, which the feeder
/// uses whenever `GEODB_RECORD_KEYS` is on). The twin used to read only the first, so every grant
/// made by the keyed writer showed up as `missing_in_geodb`. Either read failing is `Failed`.
async fn read_membership_card(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    space: &Uuid,
) -> Result<Vec<CardRow>, Verdict> {
    let own = own_space(spaces, space)?;
    let entity = entity_ref(space);
    let mut card = read_card(reader, own, &entity)
        .await
        .map_err(|_| Verdict::Failed)?;
    if let Some(gov) = spaces.system("governance") {
        card.extend(
            read_card(reader, gov, &entity)
                .await
                .map_err(|_| Verdict::Failed)?,
        );
    }
    Ok(card)
}

/// Twin of `find_editors_for_space`: the editor edges on the Space's own entity card, as a set of
/// member entities, against the editors Postgres returned.
pub async fn compare_editors(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    space: &Uuid,
    sql_editors: &[Uuid],
) -> Vec<Verdict> {
    let expected: BTreeSet<String> = sql_editors.iter().map(entity_ref).collect();
    match read_membership_card(reader, spaces, space).await {
        Ok(card) => vec![set_verdict(&expected, &refs(&card, EDITOR_PROPERTY))],
        Err(v) => vec![v],
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
    match read_membership_card(reader, spaces, space).await {
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
        Err(v) => vec![v],
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

/// What Postgres answered for a proposer lookup that found a row: `proposals.proposed_by`, and
/// `proposals.space_id` when the lookup also read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProposalValues {
    pub proposer: Uuid,
    pub space: Option<Uuid>,
}

/// Twin of `find_proposer_for_proposal` and `find_proposal_proposer_and_space`. With no row from
/// Postgres, geo-db must hold no version record either. With a row, geo-db must hold a version
/// record and its `proposer` (and, when `sql.space` is set, `space`) references must name the
/// values Postgres returned. A proposal whose records hold no such reference (written only with the
/// feeder's `GEODB_PROPOSAL_VERSION_PROPOSER_SPACE`) is `unsupported`, not a mismatch.
pub async fn compare_proposal_values(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    proposal: &Uuid,
    sql: Option<ProposalValues>,
) -> Vec<Verdict> {
    let Some(sql) = sql else {
        return compare_proposal_exists(reader, spaces, proposal, false).await;
    };
    let Some(governance) = spaces.system("governance") else {
        return vec![Verdict::Unsupported("system_space_missing")];
    };
    let result = async {
        let links = read_backlinks(reader, governance, &entity_ref(proposal)).await?;
        if links.len() >= MAX_BACKLINKS {
            return Ok(Verdict::Unsupported("too_many_links"));
        }
        let versions: Vec<String> = links
            .iter()
            .filter(|(p, _)| p == VERSION_PROPOSAL)
            .map(|(_, f)| f.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if versions.is_empty() {
            return Ok(Verdict::Mismatch("missing_in_geodb"));
        }
        let cards = read_cards(reader, governance, &versions).await?;
        let held = |property: &str| -> BTreeSet<String> {
            versions
                .iter()
                .flat_map(|id| refs(cards.get(id).map_or(&[][..], Vec::as_slice), property))
                .collect()
        };
        let mut checks = vec![(
            VERSION_PROPOSER,
            entity_ref(&sql.proposer),
            "proposer_differs",
        )];
        if let Some(space) = &sql.space {
            checks.push((VERSION_SPACE, entity_ref(space), "space_differs"));
        }
        let mut verdict = Verdict::Match;
        for (property, expected, why) in checks {
            let got = held(property);
            let one = if got.is_empty() {
                Verdict::Unsupported("values_not_stored")
            } else if got.len() == 1 && got.contains(&expected) {
                Verdict::Match
            } else {
                Verdict::Mismatch(why)
            };
            // A mismatch outranks an unsupported check, which outranks a match; the first of a kind wins.
            verdict = match (verdict, one) {
                (Verdict::Mismatch(w), _) => Verdict::Mismatch(w),
                (_, Verdict::Mismatch(w)) => Verdict::Mismatch(w),
                (Verdict::Unsupported(w), _) => Verdict::Unsupported(w),
                (Verdict::Match, other) => other,
                (other, _) => other,
            };
        }
        Ok::<_, TwinError>(verdict)
    }
    .await;
    vec![result.unwrap_or(Verdict::Failed)]
}

/// Existence only: Postgres answered a row (`sql_exists`) or none; geo-db says whether the
/// proposal has a version record. Also the no-row half of [`compare_proposal_values`].
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

/// One version record of a proposal: its `block` key and its card.
struct Version {
    block: i64,
    card: Vec<CardRow>,
}

/// The proposal's version records in the `governance` system Space, oldest block first. The inner
/// `Err` is the verdict when the records cannot decide (too many links, an unreadable block key).
async fn read_versions(
    reader: &dyn GeoDbReader,
    governance: &str,
    proposal: &Uuid,
) -> Result<Result<Vec<Version>, Verdict>, TwinError> {
    let links = read_backlinks(reader, governance, &entity_ref(proposal)).await?;
    if links.len() >= MAX_BACKLINKS {
        return Ok(Err(Verdict::Unsupported("too_many_links")));
    }
    let ids: Vec<String> = links
        .into_iter()
        .filter(|(p, _)| p == VERSION_PROPOSAL)
        .map(|(_, f)| f)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut cards = read_cards(reader, governance, &ids).await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        let card = cards.remove(id).unwrap_or_default();
        let Ok(Some(block)) = single(&card, VERSION_BLOCK) else {
            return Ok(Err(Verdict::Unsupported("ambiguous_record")));
        };
        let Some(block) = whole_number(block) else {
            return Ok(Err(Verdict::Unsupported("ambiguous_record")));
        };
        out.push(Version { block, card });
    }
    out.sort_by_key(|v| v.block);
    Ok(Ok(out))
}

/// Twin of `lookup_proposal_name`: the `name` of the proposal's latest version record against what
/// `proposals_current` answered (`None`: no row, or a NULL name).
pub async fn compare_proposal_name(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    proposal: &Uuid,
    sql_name: Option<&str>,
) -> Vec<Verdict> {
    let Some(governance) = spaces.system("governance") else {
        return vec![Verdict::Unsupported("system_space_missing")];
    };
    let result = async {
        let versions = match read_versions(reader, governance, proposal).await? {
            Ok(v) => v,
            Err(verdict) => return Ok(verdict),
        };
        let held = match versions.last() {
            None => None,
            Some(latest) => match single(&latest.card, VERSION_NAME) {
                Ok(name) => name,
                Err(()) => return Ok(Verdict::Unsupported("ambiguous_record")),
            },
        };
        Ok::<_, TwinError>(match (sql_name, held) {
            (None, None) => Verdict::Match,
            (Some(_), None) => Verdict::Mismatch("missing_in_geodb"),
            (None, Some(_)) => Verdict::Mismatch("extra_in_geodb"),
            (Some(a), Some(b)) if a == b => Verdict::Match,
            (Some(_), Some(_)) => Verdict::Mismatch("name_differs"),
        })
    }
    .await;
    vec![result.unwrap_or(Verdict::Failed)]
}

/// Twin of `lookup_vote_tallies`: yes, no and abstain counted from the proposal's vote records at
/// its current version (the number of version records), against Postgres's
/// `(yes_count, no_count, abstain_count)` (`None`: no row).
pub async fn compare_vote_tallies(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    proposal: &Uuid,
    sql: Option<(i64, i64, i64)>,
) -> Vec<Verdict> {
    let (Some(governance), Some(votes)) = (spaces.system("governance"), spaces.system("votes"))
    else {
        return vec![Verdict::Unsupported("system_space_missing")];
    };
    let result = async {
        let versions = match read_versions(reader, governance, proposal).await? {
            Ok(v) => v,
            Err(verdict) => return Ok(verdict),
        };
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
        let Some(sql) = sql else {
            return Ok(if versions.is_empty() && records.is_empty() {
                Verdict::Match
            } else {
                Verdict::Mismatch("extra_in_geodb")
            });
        };
        if versions.is_empty() {
            return Ok(Verdict::Mismatch("missing_in_geodb"));
        }
        let current = versions.len() as i64;
        let cards = read_cards(reader, votes, &records).await?;
        let mut got = (0i64, 0i64, 0i64);
        for id in &records {
            let card = cards.get(id).map_or(&[][..], Vec::as_slice);
            let (Ok(Some(version)), Ok(Some(vote))) =
                (single(card, VOTE_VERSION), single(card, VOTE_VOTE))
            else {
                return Ok(Verdict::Unsupported("ambiguous_record"));
            };
            let (Some(version), Some(vote)) = (whole_number(version), whole_number(vote)) else {
                return Ok(Verdict::Unsupported("ambiguous_record"));
            };
            if version != current {
                continue;
            }
            match vote {
                1 => got.0 += 1,
                2 => got.1 += 1,
                3 => got.2 += 1,
                _ => return Ok(Verdict::Unsupported("ambiguous_record")),
            }
        }
        Ok::<_, TwinError>(if got == sql {
            Verdict::Match
        } else {
            Verdict::Mismatch("tally_differs")
        })
    }
    .await;
    vec![result.unwrap_or(Verdict::Failed)]
}

/// Twin of `lookup_latest_block`: the feeder's applied-block watermark in `cursors` against
/// Postgres's `MAX(proposals.created_at_block)`. The two are different quantities, so the check is
/// that the watermark is not behind Postgres. No row (the feeder's `GEODB_APPLIED_BLOCK` is off, or
/// it has applied nothing) and a row that is not `<block>:<...>` are `unsupported`.
pub async fn compare_latest_block(
    cursors: &dyn CursorReader,
    sql_block: Option<u64>,
) -> Vec<Verdict> {
    let Some(sql_block) = sql_block else {
        return vec![Verdict::Unsupported("no_sql_block")];
    };
    vec![match cursors.get(APPLIED_BLOCK_KEY).await {
        Err(_) => Verdict::Failed,
        Ok(None) => Verdict::Unsupported("row_absent"),
        Ok(Some(value)) => match geodb_cursor::meta_read::block_of(&value) {
            None => Verdict::Unsupported("row_unparsable"),
            Some(geo) if geo < sql_block => Verdict::Mismatch("behind_sql"),
            Some(_) => Verdict::Match,
        },
    }]
}

/// The twin a [`crate::storage::Storage`] holds. Comparisons run one at a time on a background
/// task; a read that finds one already running counts `busy` and moves on, so the twin's cost is
/// bounded by one in-flight comparison whatever the indexer's rate.
pub struct GeoDbTwin {
    reader: Arc<dyn GeoDbReader>,
    spaces: SpaceMap,
    gate: Arc<Semaphore>,
    cursors: Option<Arc<dyn CursorReader>>,
    /// When the last `latest_block` comparison started: the read polls in a loop, so it is sampled.
    last_block_check: std::sync::Mutex<Option<std::time::Instant>>,
}

/// At most one `latest_block` comparison this often.
const LATEST_BLOCK_EVERY: Duration = Duration::from_secs(30);

/// Most rows of one expiry scan the twin judges (the scan returns up to its batch limit).
const EXPIRED_SAMPLE: usize = 8;

impl GeoDbTwin {
    pub fn new(reader: Arc<dyn GeoDbReader>, spaces: SpaceMap) -> Self {
        Self {
            reader,
            spaces,
            gate: Arc::new(Semaphore::new(1)),
            cursors: None,
            last_block_check: std::sync::Mutex::new(None),
        }
    }

    /// Give the twin a `cursors` reader, for `latest_block`. Without one that read is `unsupported`.
    pub fn with_cursors(mut self, cursors: Arc<dyn CursorReader>) -> Self {
        self.cursors = Some(cursors);
        self
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
                let mut twin = Self::new(
                    Arc::new(PipeReader::new(cfg.pipe)),
                    SpaceMap::from_path(cfg.space_map),
                );
                if let Some(c) = DerivedCursors::from_vars(|k| std::env::var(k).ok()) {
                    twin = twin.with_cursors(Arc::new(c));
                }
                Some(Arc::new(twin))
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

    /// After a proposer lookup (`read` is [`PROPOSER`] or [`PROPOSER_AND_SPACE`]) answered `sql`
    /// (`None`: no row).
    pub fn observe_proposal(
        self: &Arc<Self>,
        read: &'static str,
        proposal: Uuid,
        sql: Option<ProposalValues>,
    ) {
        self.launch(read, move |r, s| async move {
            compare_proposal_values(r.as_ref(), &s, &proposal, sql).await
        });
    }

    /// After `lookup_proposal_name(proposal)` answered `name`.
    pub fn observe_proposal_name(self: &Arc<Self>, proposal: Uuid, name: Option<&str>) {
        let name = name.map(str::to_string);
        self.launch(PROPOSAL_NAME, move |r, s| async move {
            compare_proposal_name(r.as_ref(), &s, &proposal, name.as_deref()).await
        });
    }

    /// After `lookup_vote_tallies(proposal)` answered `tallies` (yes, no, abstain).
    pub fn observe_vote_tallies(
        self: &Arc<Self>,
        proposal: Uuid,
        tallies: Option<(i64, i64, i64)>,
    ) {
        self.launch(VOTE_TALLIES, move |r, s| async move {
            compare_vote_tallies(r.as_ref(), &s, &proposal, tallies).await
        });
    }

    /// After `lookup_latest_block()` answered `block`. Sampled: it polls in a loop.
    pub fn observe_latest_block(self: &Arc<Self>, block: Option<u64>) {
        {
            let mut last = self
                .last_block_check
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < LATEST_BLOCK_EVERY) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(permit) = self.gate.clone().try_acquire_owned() else {
            skipped(LATEST_BLOCK, "busy");
            return;
        };
        let cursors = self.cursors.clone();
        rt.spawn(async move {
            let verdicts = match cursors {
                Some(c) => compare_latest_block(c.as_ref(), block).await,
                None => vec![Verdict::Unsupported("cursors_unavailable")],
            };
            record(LATEST_BLOCK, &verdicts);
            drop(permit);
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
