//! The local ONNX provider: `fastembed` over the ONNX Runtime, built from bundle bytes.
//!
//! Two sessions per slot by default — one for documents, one for queries — so a 256-text
//! document batch never sits in front of a search query. Each session is behind a mutex because
//! fastembed's `embed` takes `&mut self`; concurrency comes from the caller's semaphores, not
//! from here.

use std::sync::Mutex;

use fastembed::{
    InitOptionsUserDefined, Pooling as FastembedPooling, QuantizationMode, TextEmbedding,
    TokenizerFiles, UserDefinedEmbeddingModel,
};

use crate::descriptor::{Descriptor, Pooling, Quantization};
use crate::error::{Error, Result};
use crate::provider::{EmbeddingProvider, Purpose};

/// The bytes of a bundle's artifacts, already hash-verified by [`crate::bundle`].
pub struct BundleFiles {
    pub model: Vec<u8>,
    pub tokenizer: Vec<u8>,
    pub config: Vec<u8>,
    pub special_tokens_map: Vec<u8>,
    pub tokenizer_config: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct OnnxOptions {
    /// ONNX Runtime intra-op threads per session. `None` = the runtime's default (all cores).
    pub intra_threads: Option<usize>,
    /// Texts per inference call inside one `embed`.
    pub batch_size: usize,
    /// Load a second session dedicated to queries.
    pub query_lane: bool,
}

impl Default for OnnxOptions {
    fn default() -> Self {
        Self {
            intra_threads: None,
            batch_size: 64,
            query_lane: true,
        }
    }
}

pub struct OnnxLocalProvider {
    descriptor: Descriptor,
    options: OnnxOptions,
    documents: Mutex<TextEmbedding>,
    queries: Option<Mutex<TextEmbedding>>,
}

fn build_session(
    descriptor: &Descriptor,
    files: &BundleFiles,
    options: &OnnxOptions,
) -> Result<TextEmbedding> {
    let tokenizer_files = TokenizerFiles {
        tokenizer_file: files.tokenizer.clone(),
        config_file: files.config.clone(),
        special_tokens_map_file: files.special_tokens_map.clone(),
        tokenizer_config_file: files.tokenizer_config.clone(),
    };
    let pooling = match descriptor.pooling {
        Pooling::Cls => FastembedPooling::Cls,
        Pooling::Mean => FastembedPooling::Mean,
    };
    let quantization = match descriptor.quantization {
        Quantization::None => QuantizationMode::None,
        Quantization::Static => QuantizationMode::Static,
        Quantization::Dynamic => QuantizationMode::Dynamic,
    };
    let model = UserDefinedEmbeddingModel::new(files.model.clone(), tokenizer_files)
        .with_pooling(pooling)
        .with_quantization(quantization);
    let mut init = InitOptionsUserDefined::new().with_max_length(descriptor.max_tokens);
    if let Some(threads) = options.intra_threads {
        init = init.with_intra_threads(threads);
    }
    TextEmbedding::try_new_from_user_defined(model, init).map_err(|e| Error::Runtime(e.to_string()))
}

impl OnnxLocalProvider {
    pub fn new(descriptor: Descriptor, files: BundleFiles, options: OnnxOptions) -> Result<Self> {
        descriptor.validate()?;
        let documents = Mutex::new(build_session(&descriptor, &files, &options)?);
        let queries = if options.query_lane {
            Some(Mutex::new(build_session(&descriptor, &files, &options)?))
        } else {
            None
        };
        Ok(Self {
            descriptor,
            options,
            documents,
            queries,
        })
    }

    pub fn options(&self) -> &OnnxOptions {
        &self.options
    }

    pub fn has_query_lane(&self) -> bool {
        self.queries.is_some()
    }
}

impl EmbeddingProvider for OnnxLocalProvider {
    fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn embed(&self, texts: &[String], purpose: Purpose) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let prompt = self.descriptor.prompt(purpose);
        let prompted: Vec<String> = if prompt.is_empty() {
            texts.to_vec()
        } else {
            texts.iter().map(|t| format!("{prompt}{t}")).collect()
        };
        let lane = match (purpose, &self.queries) {
            (Purpose::Query, Some(queries)) => queries,
            _ => &self.documents,
        };
        let mut session = lane
            .lock()
            .map_err(|_| Error::Runtime("embedding session lock poisoned".into()))?;
        let vectors = session
            .embed(&prompted, Some(self.options.batch_size))
            .map_err(|e| Error::Runtime(e.to_string()))?;
        if let Some(v) = vectors
            .iter()
            .find(|v| v.len() != self.descriptor.dimensions)
        {
            return Err(Error::DimensionMismatch {
                expected: self.descriptor.dimensions,
                actual: v.len(),
            });
        }
        Ok(vectors)
    }
}
