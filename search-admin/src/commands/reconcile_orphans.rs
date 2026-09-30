//! Reconcile the search index against the knowledge graph (GEO-2548).
//!
//! `search-indexer` and `kg-indexer` consume `knowledge.edits` independently. When kg-indexer
//! loses a block (GEO-2884: a failed batch commits its offset and moves on), search-indexer has
//! still indexed it, so `/search` serves entities the graph has never heard of. Picking one in the
//! app adds a reference that resolves to nothing. On 2026-08-13 519 such entities were found and
//! pruned by hand; nothing has looked since.
//!
//! This command pages through every document in the index and checks each one against Postgres.
//! A document is an orphan when:
//!
//! - **missing-entity**: its `entity_id` has no row in `entities` (the GEO-2548 case), or
//! - **missing-in-space**: the entity exists but has no value and no outgoing relation in the
//!   document's space, which is how the API decides an entity belongs to a space
//!   (`entitySpaceFilterPlugin.ts`).
//!
//! Space-topic stub documents (`space_topic_entity_id == entity_id`) are written by search-indexer
//! from `space.topics` alone, with no edit behind them, so they are counted but never orphans.
//!
//! **Report is the default.** `--prune` deletes missing-entity orphans; missing-in-space orphans
//! are only deleted with `--prune-missing-in-space` as well. Pruning refuses to run past
//! `--max-prune` documents or `--max-orphan-ratio` of the index, because a DATABASE_URL pointing
//! at the wrong or a half-restored database would otherwise look like "every document is an
//! orphan".
//!
//! Memory and time stay bounded: the index is read with a scroll, one page at a time, and each
//! page is one Postgres statement over index lookups, far inside the 30s role statement timeout.
//! Only candidate orphans are kept, and they are re-checked after the scan so an entity that
//! search-indexer wrote a moment before kg-indexer (the two race by design) is not reported.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use opensearch::http::request::JsonBody;
use opensearch::params::Refresh;
use opensearch::{BulkParts, ClearScrollParts, OpenSearch, ScrollParts, SearchParts};
use serde_json::{Value, json};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tracing::{info, warn};
use uuid::Uuid;

use crate::opensearch_client;

/// Fields fetched per document. Everything else in `_source` is skipped to keep pages small.
const SOURCE_FIELDS: [&str; 5] = [
    "entity_id",
    "space_id",
    "space_topic_entity_id",
    "deleted",
    "name",
];

#[derive(Args)]
pub struct ReconcileOrphansCommand {
    /// Postgres URL of the knowledge graph (read only; nothing is written to Postgres).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,

    /// Index version to scan. Defaults to the alias, i.e. the index /search reads.
    #[arg(long, env = "INDEX_VERSION")]
    version: Option<u32>,

    /// Documents per scroll page, and per Postgres statement.
    #[arg(long, env = "RECONCILE_PAGE_SIZE", default_value_t = 2000)]
    page_size: usize,

    /// Seconds to wait after the scan before re-checking candidates. Covers the window where
    /// search-indexer has written a document and kg-indexer has not committed the entity yet.
    #[arg(long, env = "RECONCILE_RECHECK_DELAY_SECS", default_value_t = 30)]
    recheck_delay_secs: u64,

    /// Delete missing-entity orphans from the index. OFF by default.
    #[arg(long, env = "RECONCILE_PRUNE", default_value_t = false)]
    prune: bool,

    /// With --prune, also delete missing-in-space orphans.
    #[arg(
        long,
        env = "RECONCILE_PRUNE_MISSING_IN_SPACE",
        default_value_t = false
    )]
    prune_missing_in_space: bool,

    /// Refuse to prune more documents than this in one run.
    #[arg(long, env = "RECONCILE_MAX_PRUNE", default_value_t = 1000)]
    max_prune: usize,

    /// Refuse to prune when orphans exceed this fraction of scanned documents.
    #[arg(long, env = "RECONCILE_MAX_ORPHAN_RATIO", default_value_t = 0.01)]
    max_orphan_ratio: f64,

    /// Stop collecting candidates past this many (counting continues, pruning is refused).
    /// Bounds memory if something is badly wrong.
    #[arg(long, env = "RECONCILE_MAX_CANDIDATES", default_value_t = 100_000)]
    max_candidates: usize,

    /// Exit non-zero when confirmed orphans exceed this, so the Job fails and KubeJobFailed
    /// reaches Slack. Unset means never fail on the count.
    #[arg(long, env = "RECONCILE_FAIL_ABOVE")]
    fail_above: Option<usize>,

    /// How many individual orphans to log.
    #[arg(long, env = "RECONCILE_LOG_LIMIT", default_value_t = 200)]
    log_limit: usize,
}

/// The parts of an index document the reconcile needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DocRef {
    pub doc_id: String,
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub topic_stub: bool,
    pub soft_deleted: bool,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OrphanKind {
    MissingEntity,
    MissingInSpace,
}

impl OrphanKind {
    fn as_str(self) -> &'static str {
        match self {
            OrphanKind::MissingEntity => "missing_entity",
            OrphanKind::MissingInSpace => "missing_in_space",
        }
    }
}

/// Parse one search hit. `Err` carries the reason, for the `unparseable` count.
pub fn parse_hit(hit: &Value) -> std::result::Result<DocRef, String> {
    let doc_id = hit["_id"].as_str().ok_or("hit has no _id")?.to_string();
    let source = &hit["_source"];
    let entity_id = source["entity_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("{doc_id}: entity_id missing or not a uuid"))?;
    let space_id = source["space_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("{doc_id}: space_id missing or not a uuid"))?;
    let topic_stub = source["space_topic_entity_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        == Some(entity_id);
    Ok(DocRef {
        doc_id,
        entity_id,
        space_id,
        topic_stub,
        soft_deleted: source["deleted"].as_bool().unwrap_or(false),
        name: source["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// Decide whether a document is an orphan, given what Postgres says about it.
pub fn classify(entity_exists: bool, in_space: bool, topic_stub: bool) -> Option<OrphanKind> {
    if topic_stub {
        None
    } else if !entity_exists {
        Some(OrphanKind::MissingEntity)
    } else if !in_space {
        Some(OrphanKind::MissingInSpace)
    } else {
        None
    }
}

/// Refuse a prune that looks like a wrong database rather than a few lost blocks.
pub fn prune_gate(
    to_prune: usize,
    orphans: usize,
    scanned: u64,
    max_prune: usize,
    max_ratio: f64,
    candidates_truncated: bool,
) -> std::result::Result<(), String> {
    if candidates_truncated {
        return Err("candidate list was truncated at --max-candidates".into());
    }
    if to_prune > max_prune {
        return Err(format!(
            "{to_prune} documents to prune exceeds --max-prune {max_prune}"
        ));
    }
    if scanned == 0 {
        return Err("scanned no documents".into());
    }
    let ratio = orphans as f64 / scanned as f64;
    if ratio > max_ratio {
        return Err(format!(
            "orphan ratio {ratio:.4} exceeds --max-orphan-ratio {max_ratio}"
        ));
    }
    Ok(())
}

/// `_bulk` body deleting these document ids.
pub fn bulk_delete_body(doc_ids: &[&str]) -> Vec<JsonBody<Value>> {
    doc_ids
        .iter()
        .map(|id| json!({ "delete": { "_id": id } }).into())
        .collect()
}

#[derive(Default, Debug)]
pub struct Tally {
    pub scanned: u64,
    pub unparseable: u64,
    pub topic_stubs: u64,
    pub candidates_truncated: bool,
}

impl ReconcileOrphansCommand {
    pub async fn execute(&self, opensearch_url: &str, index_alias: &str) -> Result<()> {
        let started = Instant::now();
        let target = match self.version {
            Some(v) => format!("{index_alias}_v{v}"),
            None => index_alias.to_string(),
        };
        let mode = if !self.prune {
            "report"
        } else if self.prune_missing_in_space {
            "prune(missing_entity,missing_in_space)"
        } else {
            "prune(missing_entity)"
        };
        info!(index = %target, mode, page_size = self.page_size, "Starting orphan reconcile");

        let client = opensearch_client::create_client(opensearch_url)?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&self.database_url)
            .await
            .context("Failed to connect to Postgres")?;

        // A reachable but empty (or wrong) database would make every document an orphan.
        let has_entities: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.entities)")
                .fetch_one(&pool)
                .await
                .context("Failed to read entities")?;
        if !has_entities {
            bail!("entities table is empty: refusing to reconcile against this database");
        }

        let (tally, candidates) = self.scan(&client, &pool, &target).await?;
        info!(
            scanned = tally.scanned,
            candidates = candidates.len(),
            elapsed_s = started.elapsed().as_secs(),
            "Scan complete; re-checking candidates"
        );

        if !candidates.is_empty() && self.recheck_delay_secs > 0 {
            tokio::time::sleep(Duration::from_secs(self.recheck_delay_secs)).await;
        }
        let mut confirmed: Vec<(DocRef, OrphanKind)> = Vec::with_capacity(candidates.len());
        for chunk in candidates.chunks(self.page_size.max(1)) {
            let docs: Vec<DocRef> = chunk.iter().map(|(d, _)| d.clone()).collect();
            confirmed.extend(check_page(&pool, &docs).await?);
        }
        let transient = candidates.len() - confirmed.len();

        self.report(&tally, &confirmed, transient, started);

        if self.prune {
            self.prune_confirmed(&client, &target, &tally, &confirmed)
                .await?;
        }

        if let Some(limit) = self.fail_above
            && confirmed.len() > limit
        {
            bail!(
                "{} confirmed orphans exceeds RECONCILE_FAIL_ABOVE={limit}",
                confirmed.len()
            );
        }
        Ok(())
    }

    async fn scan(
        &self,
        client: &OpenSearch,
        pool: &PgPool,
        target: &str,
    ) -> Result<(Tally, Vec<(DocRef, OrphanKind)>)> {
        let mut tally = Tally::default();
        let mut candidates: Vec<(DocRef, OrphanKind)> = Vec::new();

        let response = client
            .search(SearchParts::Index(&[target]))
            .scroll("5m")
            .body(json!({
                "size": self.page_size,
                "sort": ["_doc"],
                "_source": SOURCE_FIELDS,
                "query": { "match_all": {} }
            }))
            .send()
            .await
            .context("Failed to start scroll")?;
        let mut page = read_json(response, "start scroll").await?;
        let total = page["hits"]["total"]["value"].as_u64().unwrap_or(0);
        info!(total, "Index documents to scan");

        let mut scroll_id: Option<String> = page["_scroll_id"].as_str().map(str::to_string);
        let result: Result<()> = async {
            loop {
                let hits = page["hits"]["hits"].as_array().cloned().unwrap_or_default();
                if hits.is_empty() {
                    break;
                }
                let mut docs = Vec::with_capacity(hits.len());
                for hit in &hits {
                    tally.scanned += 1;
                    match parse_hit(hit) {
                        Ok(doc) if doc.topic_stub => tally.topic_stubs += 1,
                        Ok(doc) => docs.push(doc),
                        Err(reason) => {
                            tally.unparseable += 1;
                            if tally.unparseable <= 20 {
                                warn!(reason, "Unparseable index document");
                            }
                        }
                    }
                }
                for found in check_page(pool, &docs).await? {
                    if candidates.len() < self.max_candidates {
                        candidates.push(found);
                    } else {
                        tally.candidates_truncated = true;
                    }
                }
                if tally.scanned % 100_000 < hits.len() as u64 {
                    info!(
                        scanned = tally.scanned,
                        total,
                        candidates = candidates.len(),
                        "Progress"
                    );
                }

                let Some(id) = scroll_id.clone() else { break };
                let response = client
                    .scroll(ScrollParts::None)
                    .body(json!({ "scroll": "5m", "scroll_id": id }))
                    .send()
                    .await
                    .context("Failed to continue scroll")?;
                page = read_json(response, "continue scroll").await?;
                if let Some(id) = page["_scroll_id"].as_str() {
                    scroll_id = Some(id.to_string());
                }
            }
            Ok(())
        }
        .await;

        if let Some(id) = scroll_id {
            // Best effort: an abandoned scroll context expires on its own after 5 minutes.
            let _ = client
                .clear_scroll(ClearScrollParts::None)
                .body(json!({ "scroll_id": [id] }))
                .send()
                .await;
        }
        result?;
        Ok((tally, candidates))
    }

    fn report(
        &self,
        tally: &Tally,
        confirmed: &[(DocRef, OrphanKind)],
        transient: usize,
        started: Instant,
    ) {
        let mut by_kind: BTreeMap<OrphanKind, usize> = BTreeMap::new();
        let mut by_space: HashMap<Uuid, usize> = HashMap::new();
        let mut named = 0usize;
        let mut soft_deleted = 0usize;
        for (doc, kind) in confirmed {
            *by_kind.entry(*kind).or_default() += 1;
            *by_space.entry(doc.space_id).or_default() += 1;
            named += doc.name.is_some() as usize;
            soft_deleted += doc.soft_deleted as usize;
        }
        let mut spaces: Vec<(Uuid, usize)> = by_space.into_iter().collect();
        spaces.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let top_spaces = spaces
            .iter()
            .take(10)
            .map(|(s, n)| format!("{}:{n}", s.simple()))
            .collect::<Vec<_>>()
            .join(",");

        for (doc, kind) in confirmed.iter().take(self.log_limit) {
            info!(
                doc_id = %doc.doc_id,
                kind = kind.as_str(),
                soft_deleted = doc.soft_deleted,
                name = doc.name.as_deref().unwrap_or(""),
                "orphan"
            );
        }

        // One line with every number, so the run can be read (or grepped) as a metric.
        info!(
            scanned = tally.scanned,
            orphans = confirmed.len(),
            missing_entity = by_kind.get(&OrphanKind::MissingEntity).copied().unwrap_or(0),
            missing_in_space = by_kind.get(&OrphanKind::MissingInSpace).copied().unwrap_or(0),
            named,
            soft_deleted,
            spaces = spaces.len(),
            top_spaces = %top_spaces,
            transient,
            topic_stubs = tally.topic_stubs,
            unparseable = tally.unparseable,
            candidates_truncated = tally.candidates_truncated,
            elapsed_s = started.elapsed().as_secs(),
            "search_orphan_reconcile_summary"
        );
    }

    async fn prune_confirmed(
        &self,
        client: &OpenSearch,
        target: &str,
        tally: &Tally,
        confirmed: &[(DocRef, OrphanKind)],
    ) -> Result<()> {
        let doomed: Vec<&DocRef> = confirmed
            .iter()
            .filter(|(_, k)| {
                *k == OrphanKind::MissingEntity
                    || (self.prune_missing_in_space && *k == OrphanKind::MissingInSpace)
            })
            .map(|(d, _)| d)
            .collect();
        if doomed.is_empty() {
            info!("Nothing to prune");
            return Ok(());
        }
        if let Err(reason) = prune_gate(
            doomed.len(),
            confirmed.len(),
            tally.scanned,
            self.max_prune,
            self.max_orphan_ratio,
            tally.candidates_truncated,
        ) {
            bail!("Refusing to prune: {reason}");
        }

        let mut deleted = 0u64;
        let mut not_found = 0u64;
        let mut failed = 0u64;
        for chunk in doomed.chunks(500) {
            let ids: Vec<&str> = chunk.iter().map(|d| d.doc_id.as_str()).collect();
            let response = client
                .bulk(BulkParts::Index(target))
                // Wait for the deletes to become visible, so a run straight after this one
                // (or /search) already sees them gone.
                .refresh(Refresh::WaitFor)
                .body(bulk_delete_body(&ids))
                .send()
                .await
                .context("Bulk delete failed")?;
            let body = read_json(response, "bulk delete").await?;
            for item in body["items"].as_array().into_iter().flatten() {
                let status = item["delete"]["status"].as_u64().unwrap_or(0);
                if (200..300).contains(&status) {
                    deleted += 1;
                } else if status == 404 {
                    // Already gone, which is the outcome we wanted.
                    not_found += 1;
                } else {
                    failed += 1;
                    warn!(item = %item, "Delete failed");
                }
            }
        }
        info!(deleted, not_found, failed, "search_orphan_reconcile_pruned");
        if failed > 0 {
            bail!("{failed} deletes failed");
        }
        Ok(())
    }
}

/// Check one page of documents against Postgres, returning the orphans in it.
///
/// One statement per page. Every probe is an index lookup (`entities_pkey`,
/// `values_entity_space_idx`, `relations_from_entity_space_idx`), so a 2000-document page costs
/// milliseconds, not the 30s role timeout.
async fn check_page(pool: &PgPool, docs: &[DocRef]) -> Result<Vec<(DocRef, OrphanKind)>> {
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let idx: Vec<i32> = (0..docs.len() as i32).collect();
    let entity_ids: Vec<Uuid> = docs.iter().map(|d| d.entity_id).collect();
    let space_ids: Vec<Uuid> = docs.iter().map(|d| d.space_id).collect();
    let rows: Vec<(i32, bool, bool)> = sqlx::query_as(
        r#"
        SELECT * FROM (
            SELECT d.idx,
                   EXISTS (SELECT 1 FROM public.entities e WHERE e.id = d.entity_id) AS entity_exists,
                   (EXISTS (SELECT 1 FROM public.values v
                            WHERE v.entity_id = d.entity_id AND v.space_id = d.space_id)
                    OR EXISTS (SELECT 1 FROM public.relations r
                               WHERE r.from_entity_id = d.entity_id AND r.space_id = d.space_id)
                   ) AS in_space
            FROM unnest($1::int[], $2::uuid[], $3::uuid[]) AS d(idx, entity_id, space_id)
        ) checked
        WHERE NOT (entity_exists AND in_space)
        "#,
    )
    .bind(&idx)
    .bind(&entity_ids)
    .bind(&space_ids)
    .fetch_all(pool)
    .await
    .context("Postgres orphan check failed")?;

    Ok(rows
        .into_iter()
        .filter_map(|(i, entity_exists, in_space)| {
            let doc = docs.get(i as usize)?;
            classify(entity_exists, in_space, doc.topic_stub).map(|k| (doc.clone(), k))
        })
        .collect())
}

async fn read_json(response: opensearch::http::response::Response, what: &str) -> Result<Value> {
    let status = response.status_code();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("{what} failed with status {status}: {body}");
    }
    response
        .json()
        .await
        .with_context(|| format!("Failed to parse {what} response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: &str = "13711444-bd8d-405f-9985-2b523d112166";
    const S: &str = "c9f267dc-b0d2-7071-8c2a-3c45a64afd32";

    fn hit(source: Value) -> Value {
        json!({ "_id": format!("{E}_{S}"), "_source": source })
    }

    #[test]
    fn parses_a_normal_document() {
        let doc = parse_hit(&hit(json!({
            "entity_id": E, "space_id": S, "name": "Lebanese President"
        })))
        .unwrap();
        assert_eq!(doc.doc_id, format!("{E}_{S}"));
        assert_eq!(doc.entity_id, Uuid::parse_str(E).unwrap());
        assert_eq!(doc.space_id, Uuid::parse_str(S).unwrap());
        assert!(!doc.topic_stub);
        assert!(!doc.soft_deleted);
        assert_eq!(doc.name.as_deref(), Some("Lebanese President"));
    }

    #[test]
    fn a_missing_deleted_field_is_not_deleted() {
        // 81% of live documents have no `deleted` field (GEO-2548, 2026-08-13).
        let doc = parse_hit(&hit(json!({ "entity_id": E, "space_id": S }))).unwrap();
        assert!(!doc.soft_deleted);
        assert_eq!(doc.name, None);
        let doc = parse_hit(&hit(
            json!({ "entity_id": E, "space_id": S, "deleted": true }),
        ))
        .unwrap();
        assert!(doc.soft_deleted);
    }

    #[test]
    fn empty_name_counts_as_unnamed() {
        let doc = parse_hit(&hit(json!({ "entity_id": E, "space_id": S, "name": "" }))).unwrap();
        assert_eq!(doc.name, None);
    }

    #[test]
    fn detects_a_topic_stub_only_when_the_topic_is_the_entity_itself() {
        let stub = parse_hit(&hit(json!({
            "entity_id": E, "space_id": S, "space_topic_entity_id": E
        })))
        .unwrap();
        assert!(stub.topic_stub);
        let ordinary = parse_hit(&hit(json!({
            "entity_id": E, "space_id": S,
            "space_topic_entity_id": "72a20422-c84d-4ec3-be30-a6cb962d1801"
        })))
        .unwrap();
        assert!(!ordinary.topic_stub);
    }

    #[test]
    fn rejects_documents_without_usable_ids() {
        assert!(parse_hit(&json!({ "_source": { "entity_id": E, "space_id": S } })).is_err());
        assert!(parse_hit(&hit(json!({ "space_id": S }))).is_err());
        assert!(parse_hit(&hit(json!({ "entity_id": "nope", "space_id": S }))).is_err());
        assert!(parse_hit(&hit(json!({ "entity_id": E }))).is_err());
    }

    #[test]
    fn classifies_orphans() {
        assert_eq!(classify(true, true, false), None);
        assert_eq!(
            classify(false, false, false),
            Some(OrphanKind::MissingEntity)
        );
        // No entities row wins even if a stray value somehow exists.
        assert_eq!(
            classify(false, true, false),
            Some(OrphanKind::MissingEntity)
        );
        assert_eq!(
            classify(true, false, false),
            Some(OrphanKind::MissingInSpace)
        );
    }

    #[test]
    fn topic_stubs_are_never_orphans() {
        assert_eq!(classify(false, false, true), None);
        assert_eq!(classify(true, false, true), None);
    }

    #[test]
    fn prune_gate_allows_a_small_orphan_set() {
        assert!(prune_gate(519, 519, 2_104_273, 1000, 0.01, false).is_ok());
    }

    #[test]
    fn prune_gate_refuses_what_looks_like_a_wrong_database() {
        // Every document an orphan: the ratio check stops it even under a huge --max-prune.
        let err = prune_gate(2_000_000, 2_000_000, 2_000_000, usize::MAX, 0.01, false);
        assert!(err.unwrap_err().contains("ratio"));
    }

    #[test]
    fn prune_gate_refuses_past_max_prune() {
        let err = prune_gate(1001, 1001, 10_000_000, 1000, 0.01, false);
        assert!(err.unwrap_err().contains("--max-prune"));
    }

    #[test]
    fn prune_gate_refuses_truncated_or_empty_scans() {
        assert!(prune_gate(1, 1, 1_000_000, 1000, 0.01, true).is_err());
        assert!(prune_gate(0, 0, 0, 1000, 0.01, false).is_err());
    }

    #[test]
    fn ratio_counts_every_orphan_not_only_the_prunable_ones() {
        // 5 prunable but 200 orphans of 10k scanned is 2%: refuse.
        assert!(prune_gate(5, 200, 10_000, 1000, 0.01, false).is_err());
    }

    #[test]
    fn bulk_body_has_one_delete_action_per_id() {
        let body = bulk_delete_body(&["a_b", "c_d"]);
        assert_eq!(body.len(), 2);
    }
}
