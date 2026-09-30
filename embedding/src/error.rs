use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("bundle {dir}: {reason}")]
    Bundle { dir: PathBuf, reason: String },
    #[error(
        "bundle {dir}: artifact {file} hash mismatch: descriptor says {expected}, file is {actual}"
    )]
    HashMismatch {
        dir: PathBuf,
        file: String,
        expected: String,
        actual: String,
    },
    #[error(
        "bundle {dir}: slot id {computed} (derived from bundle.json) does not match the directory name {dir_name}"
    )]
    SlotMismatch {
        dir: PathBuf,
        computed: String,
        dir_name: String,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("model runtime: {0}")]
    Runtime(String),
    #[error("provider produced {actual} dimensions, descriptor says {expected}")]
    DimensionMismatch { expected: usize, actual: usize },
    #[error("unknown text template {0:?}")]
    UnknownTemplate(String),
    #[error("unsupported artifact source {0:?} (expected \"hf:<repo>@<revision>\")")]
    UnsupportedSource(String),
}

pub type Result<T> = std::result::Result<T, Error>;
