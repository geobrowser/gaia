use clap::Parser;

/// Every flag has an environment variable, matching the other indexers.
#[derive(Parser, Clone, Debug)]
#[command(
    name = "embedding-indexer",
    about = "Keeps emb_<slot> on every in-scope search document equal to the embedding of its current text"
)]
pub struct Config {
    #[arg(long, env = "OPENSEARCH_URL", default_value = "http://localhost:9200")]
    pub opensearch_url: String,

    /// Base alias; ENVIRONMENT prefixes it (staging_ / testnet_ / none) like every other search crate.
    #[arg(long, env = "INDEX_ALIAS", default_value = "entities")]
    pub index_alias: String,

    /// Act on this exact index instead of the prefixed alias.
    #[arg(long, env = "EMBED_INDEX")]
    pub index: Option<String>,

    /// Control index holding the per-slot checkpoint (default: `<prefix>search_control`).
    #[arg(long, env = "EMBED_CONTROL_INDEX")]
    pub control_index: Option<String>,

    #[arg(long, env = "EMBEDDING_SERVICE_URL")]
    pub service_url: String,

    /// The slot this process maintains; must be registered on the index and loaded by the service.
    #[arg(long, env = "EMBEDDING_SLOT")]
    pub slot: String,

    #[arg(long, env = "EMBED_POLL_INTERVAL_MS", default_value_t = 5_000)]
    pub poll_interval_ms: u64,

    /// Follow mode re-reads documents stamped within this many seconds before the checkpoint.
    #[arg(long, env = "EMBED_OVERLAP_S", default_value_t = 30)]
    pub overlap_s: u64,

    #[arg(long, env = "EMBED_PAGE_SIZE", default_value_t = 500)]
    pub page_size: usize,

    /// Texts per call to the embedding service (its own limit is 256).
    #[arg(long, env = "EMBED_BATCH_SIZE", default_value_t = 64)]
    pub batch_size: usize,

    /// Bound on documents scanned per cycle so follow mode never starves behind a backfill.
    #[arg(long, env = "EMBED_MAX_DOCS_PER_CYCLE", default_value_t = 20_000)]
    pub max_docs_per_cycle: usize,

    /// Type entity ids (dashed or dashless) a document must carry; empty = every named document.
    #[arg(long, env = "EMBED_SCOPE_TYPE_IDS", value_delimiter = ',', num_args = 0..)]
    pub scope_type_ids: Vec<String>,

    /// Space ids a document must belong to; empty = any.
    #[arg(long, env = "EMBED_SCOPE_SPACE_IDS", value_delimiter = ',', num_args = 0..)]
    pub scope_space_ids: Vec<String>,

    /// Leave soft-deleted documents alone (they keep their last vector; the search filter hides them).
    #[arg(long, env = "EMBED_SKIP_DELETED", default_value_t = true, action = clap::ArgAction::Set)]
    pub skip_deleted: bool,

    /// Text-hash → vector cache, so the same claim in several spaces is embedded once.
    #[arg(long, env = "EMBED_LRU_SIZE", default_value_t = 100_000)]
    pub lru_size: usize,

    /// Backend for the initial full scan. v1 knows `service`; `extraction_api` is reserved (design D11).
    #[arg(long, env = "EMBED_BACKFILL_BACKEND", default_value = "service")]
    pub backfill_backend: String,

    /// Finish the backfill (if any) and one follow cycle, then exit 0. For jobs and tests.
    #[arg(long, env = "EMBED_ONCE", default_value_t = false)]
    pub once: bool,

    #[arg(long, env = "HEALTH_PORT", default_value_t = 8080)]
    pub health_port: u16,
}

impl Config {
    /// The index to read and write. With no override, the environment-prefixed alias.
    pub fn resolved_index(&self) -> String {
        self.index.clone().unwrap_or_else(|| {
            format!(
                "{}{}",
                search_indexer_shared::get_index_prefix(),
                self.index_alias
            )
        })
    }

    pub fn resolved_control_index(&self) -> String {
        self.control_index.clone().unwrap_or_else(|| {
            format!(
                "{}search_control",
                search_indexer_shared::get_index_prefix()
            )
        })
    }
}
