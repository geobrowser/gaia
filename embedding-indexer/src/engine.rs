//! The loop: backfill, then follow. One page at a time; the control document is persisted only
//! after a page has been fully applied, so a crash redoes work and never skips any.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::time::Instant;

use chrono::Utc;
use embedding::Descriptor;
use lru::LruCache;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::config::Config;
use crate::error::{IndexerError, Result};
use crate::queries::{self, SlotFields};
use crate::scope::{Action, Doc, Scope, classify, parse_hit};
use crate::service::ServiceClient;
use crate::store::{ControlDoc, Mode, Store};

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CycleStats {
    pub mode: String,
    pub pages: usize,
    pub scanned: usize,
    pub unchanged: usize,
    pub embedded: usize,
    pub removed: usize,
    pub cas_noop: usize,
    pub missing: usize,
    pub skipped: usize,
    pub failed: usize,
    pub lru_hits: usize,
    pub service_ms: u64,
    pub opensearch_ms: u64,
    pub wall_ms: u64,
    pub backfill_complete: bool,
}

pub struct Engine {
    cfg: Config,
    store: Store,
    service: ServiceClient,
    scope: Scope,
    descriptor: Descriptor,
    fields: SlotFields,
    control: ControlDoc,
    lru: LruCache<String, Vec<f32>>,
    /// doc id → text hash of the vector this process wrote most recently. A document re-read
    /// before OpenSearch refreshed (the overlap window, the backfill→follow handover) still shows
    /// the old hash; if its text hashes to what was just written, there is nothing to do.
    recent_writes: LruCache<String, String>,
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Engine {
    /// Resolve the index, read the slot's descriptor from `_meta`, verify the service serves the
    /// same descriptor, and load or initialize the control document.
    pub async fn start(cfg: Config) -> Result<Self> {
        if cfg.backfill_backend != "service" {
            return Err(IndexerError::Fatal(format!(
                "EMBED_BACKFILL_BACKEND={:?} is not implemented in this version (only `service`)",
                cfg.backfill_backend
            )));
        }
        let scope = Scope::from_config(&cfg)?;
        let store = Store::new(
            &cfg.opensearch_url,
            &cfg.resolved_index(),
            &cfg.resolved_control_index(),
        )?;
        let (descriptor, mapped) = store.slot_descriptor(&cfg.slot).await?;
        if !mapped {
            return Err(IndexerError::Fatal(format!(
                "slot {} is registered in _meta but {} is not mapped on {}",
                cfg.slot,
                descriptor.vector_field(),
                store.index
            )));
        }
        let service = ServiceClient::new(
            &cfg.service_url,
            &cfg.slot,
            &descriptor.hash(),
            descriptor.dimensions,
        )?;
        service.verify().await?;
        store.ensure_control_index().await?;
        let control = match store.load_control(&cfg.slot).await? {
            Some(c) => c,
            None => {
                let c = ControlDoc {
                    slot: cfg.slot.clone(),
                    mode: Mode::Backfill,
                    checkpoint_ms: None,
                    backfill_after: None,
                    backfill_started_ms: Some(now_ms()),
                    updated_at: now_rfc3339(),
                    last_cycle: json!({}),
                };
                store.save_control(&c).await?;
                c
            }
        };
        info!(
            slot = %cfg.slot,
            index = %store.index,
            model = %descriptor.model_id,
            dimensions = descriptor.dimensions,
            mode = ?control.mode,
            checkpoint_ms = ?control.checkpoint_ms,
            scope_types = scope.type_ids.len(),
            scope_spaces = scope.space_ids.len(),
            "embedding-indexer started"
        );
        let fields = SlotFields::for_vector_field(&descriptor.vector_field());
        let lru = LruCache::new(NonZeroUsize::new(cfg.lru_size.max(1)).expect("non-zero"));
        let recent_writes =
            LruCache::new(NonZeroUsize::new(cfg.lru_size.max(1)).expect("non-zero"));
        Ok(Self {
            cfg,
            store,
            service,
            scope,
            descriptor,
            fields,
            control,
            lru,
            recent_writes,
        })
    }

    pub fn mode(&self) -> Mode {
        self.control.mode
    }

    /// One cycle: a bounded slice of the backfill, or one follow pass over everything stamped
    /// since the checkpoint (minus overlap).
    pub async fn cycle(&mut self) -> Result<CycleStats> {
        let started = Instant::now();
        let mut stats = CycleStats {
            mode: format!("{:?}", self.control.mode).to_lowercase(),
            ..Default::default()
        };
        match self.control.mode {
            Mode::Backfill => self.backfill_slice(&mut stats).await?,
            Mode::Follow => self.follow_pass(&mut stats).await?,
        }
        stats.wall_ms = started.elapsed().as_millis() as u64;
        self.control.last_cycle = serde_json::to_value(&stats).unwrap_or(json!({}));
        self.control.updated_at = now_rfc3339();
        self.store.save_control(&self.control).await?;
        info!(
            mode = %stats.mode, pages = stats.pages, scanned = stats.scanned, unchanged = stats.unchanged,
            embedded = stats.embedded, removed = stats.removed, cas_noop = stats.cas_noop, missing = stats.missing,
            skipped = stats.skipped, failed = stats.failed, lru_hits = stats.lru_hits,
            service_ms = stats.service_ms, opensearch_ms = stats.opensearch_ms, wall_ms = stats.wall_ms,
            backfill_complete = stats.backfill_complete,
            "embedding_indexer.cycle_end"
        );
        Ok(stats)
    }

    /// Backfill to completion, then one follow pass. For `--once` and tests. Transient errors
    /// (a 429 under disk pressure, a service restart) are retried with backoff — a Job must not
    /// die on one of those — up to `RUN_ONCE_MAX_TRANSIENT` in a row; fatal errors return at once.
    pub async fn run_once(&mut self) -> Result<Vec<CycleStats>> {
        const RUN_ONCE_MAX_TRANSIENT: u32 = 10;
        let mut all = Vec::new();
        let mut failures = 0u32;
        let mut backoff = std::time::Duration::from_secs(1);
        loop {
            match self.cycle().await {
                Ok(stats) => {
                    failures = 0;
                    backoff = std::time::Duration::from_secs(1);
                    let was_backfill = stats.mode == "backfill";
                    all.push(stats);
                    if !was_backfill {
                        return Ok(all);
                    }
                }
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => {
                    failures += 1;
                    if failures >= RUN_ONCE_MAX_TRANSIENT {
                        return Err(e);
                    }
                    warn!(error = %e, attempt = failures, retry_in_s = backoff.as_secs(), "transient error; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
                }
            }
        }
    }

    async fn backfill_slice(&mut self, stats: &mut CycleStats) -> Result<()> {
        let vf = self.fields.vec.clone();
        while stats.scanned < self.cfg.max_docs_per_cycle {
            let body = queries::backfill_query(
                &self.scope,
                &vf,
                self.control.backfill_after.as_ref(),
                self.cfg.page_size,
            );
            let (hits, took) = self.store.search(body).await?;
            stats.opensearch_ms += took;
            stats.pages += 1;
            if hits.is_empty() {
                break;
            }
            let docs: Vec<Doc> = hits.iter().map(|h| parse_hit(h, &vf)).collect();
            let last_sort = docs.last().and_then(|d| d.sort.clone());
            let n = docs.len();
            self.process_page(docs, stats).await?;
            self.control.backfill_after = last_sort;
            self.control.updated_at = now_rfc3339();
            self.store.save_control(&self.control).await?;
            if n < self.cfg.page_size {
                break;
            }
        }
        // The scan ended (short page or nothing) rather than the per-cycle bound: switch to follow,
        // anchored at the backfill start so anything stamped during the scan is revisited.
        if stats.scanned < self.cfg.max_docs_per_cycle {
            let started = self.control.backfill_started_ms.unwrap_or_else(now_ms);
            self.control.mode = Mode::Follow;
            self.control.checkpoint_ms = Some(started);
            self.control.backfill_after = None;
            stats.backfill_complete = true;
            info!(
                checkpoint_ms = started,
                "backfill complete; switching to follow"
            );
        }
        Ok(())
    }

    async fn follow_pass(&mut self, stats: &mut CycleStats) -> Result<()> {
        let vf = self.fields.vec.clone();
        let checkpoint = self.control.checkpoint_ms.unwrap_or_else(now_ms);
        let since = checkpoint - (self.cfg.overlap_s as i64) * 1000;
        let mut after: Option<Value> = None;
        let mut max_seen = checkpoint;
        while stats.scanned < self.cfg.max_docs_per_cycle {
            let body = queries::follow_query(&vf, since, after.as_ref(), self.cfg.page_size);
            let (hits, took) = self.store.search(body).await?;
            stats.opensearch_ms += took;
            stats.pages += 1;
            if hits.is_empty() {
                break;
            }
            let docs: Vec<Doc> = hits.iter().map(|h| parse_hit(h, &vf)).collect();
            for d in &docs {
                if let Some(ms) = d
                    .sort
                    .as_ref()
                    .and_then(|s| s.get(0))
                    .and_then(Value::as_i64)
                {
                    max_seen = max_seen.max(ms);
                }
            }
            after = docs.last().and_then(|d| d.sort.clone());
            let n = docs.len();
            self.process_page(docs, stats).await?;
            if n < self.cfg.page_size {
                break;
            }
        }
        self.control.checkpoint_ms = Some(max_seen);
        Ok(())
    }

    /// Classify a page, remove what is out of scope, embed what changed, CAS-write the vectors.
    async fn process_page(&mut self, docs: Vec<Doc>, stats: &mut CycleStats) -> Result<()> {
        stats.scanned += docs.len();
        let mut removes: Vec<(String, Value)> = Vec::new();
        let mut to_embed: Vec<(Doc, String, String)> = Vec::new(); // doc, text, hash
        for doc in docs {
            match classify(&doc, &self.scope, &self.descriptor.text_template)? {
                Action::Skip(_) => stats.skipped += 1,
                Action::Unchanged => stats.unchanged += 1,
                Action::Remove => {
                    removes.push((doc.id.clone(), queries::remove_body(&self.fields)))
                }
                Action::Embed { text, hash } => {
                    if self.recent_writes.peek(&doc.id) == Some(&hash) {
                        stats.unchanged += 1;
                    } else {
                        to_embed.push((doc, text, hash));
                    }
                }
            }
        }
        if !removes.is_empty() {
            let out = self.store.bulk_update(&removes).await?;
            stats.removed += out.updated;
            stats.missing += out.missing;
            stats.failed += out.failed.len();
            stats.opensearch_ms += out.took_ms;
        }
        if to_embed.is_empty() {
            return Ok(());
        }

        // Unique texts: the same claim in several spaces is embedded once; the LRU spans pages.
        let mut vectors: HashMap<String, Vec<f32>> = HashMap::new();
        let mut pending: Vec<(String, String)> = Vec::new(); // hash, text
        for (_, text, hash) in &to_embed {
            if vectors.contains_key(hash) || pending.iter().any(|(h, _)| h == hash) {
                continue;
            }
            if let Some(v) = self.lru.get(hash) {
                vectors.insert(hash.clone(), v.clone());
                stats.lru_hits += 1;
            } else {
                pending.push((hash.clone(), text.clone()));
            }
        }
        for chunk in pending.chunks(self.cfg.batch_size.max(1)) {
            let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
            let t = Instant::now();
            let got = self.service.embed_documents(&texts).await?;
            stats.service_ms += t.elapsed().as_millis() as u64;
            for ((hash, _), v) in chunk.iter().zip(got) {
                self.lru.put(hash.clone(), v.clone());
                vectors.insert(hash.clone(), v);
            }
        }

        let now = now_rfc3339();
        let writes: Vec<(String, Value)> = to_embed
            .iter()
            .filter_map(|(doc, _, hash)| {
                let v = vectors.get(hash)?;
                Some((
                    doc.id.clone(),
                    queries::cas_write_body(
                        &self.fields,
                        doc.name.as_deref(),
                        doc.description.as_deref(),
                        v,
                        hash,
                        &now,
                    ),
                ))
            })
            .collect();
        for (doc, _, hash) in &to_embed {
            self.recent_writes.put(doc.id.clone(), hash.clone());
        }
        for chunk in writes.chunks(200) {
            let out = self.store.bulk_update(chunk).await?;
            stats.embedded += out.updated;
            stats.cas_noop += out.noop;
            stats.missing += out.missing;
            stats.failed += out.failed.len();
            stats.opensearch_ms += out.took_ms;
            if out.noop > 0 {
                warn!(
                    noop = out.noop,
                    "text changed between read and write; the next poll retries"
                );
            }
        }
        Ok(())
    }
}
