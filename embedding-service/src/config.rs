use std::path::PathBuf;

/// `serve` settings. Every flag has an `EMBEDDING_*` environment variable (or `PORT`).
#[derive(clap::Parser, Clone, Debug)]
pub struct ServeArgs {
    /// Directory holding one `<slot>/bundle.json` per model to serve.
    #[arg(long, env = "EMBEDDING_MODELS_DIR", default_value = "/models")]
    pub models_dir: PathBuf,

    /// Slot ids to load (comma-separated). Empty = every bundle found under the models directory.
    #[arg(long, env = "EMBEDDING_SLOTS", value_delimiter = ',', num_args = 0..)]
    pub slots: Vec<String>,

    /// ONNX Runtime intra-op threads per session. Default: the runtime's (all cores).
    #[arg(long, env = "EMBEDDING_INTRA_OP_THREADS")]
    pub intra_threads: Option<usize>,

    /// Texts per inference call.
    #[arg(long, env = "EMBEDDING_BATCH_SIZE", default_value_t = 64)]
    pub batch_size: usize,

    /// Largest `texts` array one request may carry (413 above it).
    #[arg(long, env = "EMBEDDING_MAX_BATCH", default_value_t = 256)]
    pub max_batch: usize,

    /// Longest single text, in characters (413 above it; the tokenizer truncates to max_tokens).
    #[arg(long, env = "EMBEDDING_MAX_TEXT_CHARS", default_value_t = 8000)]
    pub max_text_chars: usize,

    /// Concurrent document requests admitted to the model.
    #[arg(long, env = "EMBEDDING_MAX_INFLIGHT", default_value_t = 4)]
    pub max_inflight: usize,

    /// Concurrent query requests admitted to the query lane.
    #[arg(long, env = "EMBEDDING_QUERY_INFLIGHT", default_value_t = 2)]
    pub query_inflight: usize,

    /// Load a second session per slot so queries never wait behind document batches.
    #[arg(long, env = "EMBEDDING_QUERY_LANE", default_value_t = true, action = clap::ArgAction::Set)]
    pub query_lane: bool,

    #[arg(long, env = "EMBEDDING_BIND", default_value = "0.0.0.0")]
    pub bind: String,

    #[arg(long, env = "PORT", default_value_t = 8080)]
    pub port: u16,
}
