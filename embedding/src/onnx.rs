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
        let lengths: Vec<usize> = prompted.iter().map(|t| t.len()).collect();
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); prompted.len()];
        for batch in plan_batches(
            &lengths,
            self.options.batch_size,
            self.options.batch_size * CHARS_PER_BATCH_SLOT,
        ) {
            let texts: Vec<&str> = batch.iter().map(|&i| prompted[i].as_str()).collect();
            let vectors = session
                .embed(&texts, Some(batch.len()))
                .map_err(|e| Error::Runtime(e.to_string()))?;
            if vectors.len() != batch.len() {
                return Err(Error::Runtime(format!(
                    "runtime returned {} vectors for {} texts",
                    vectors.len(),
                    batch.len()
                )));
            }
            for (slot, vector) in batch.iter().zip(vectors) {
                if vector.len() != self.descriptor.dimensions {
                    return Err(Error::DimensionMismatch {
                        expected: self.descriptor.dimensions,
                        actual: vector.len(),
                    });
                }
                out[*slot] = vector;
            }
        }
        Ok(out)
    }
}

/// Characters of text a batch "slot" is budgeted for: a batch of `batch_size` texts may hold
/// `batch_size × this` characters of its *longest* text times its count. 256 chars ≈ 64 tokens
/// for English, so a batch of 64 short names runs whole, while a 2,000-character description
/// runs in a batch of eight and a 10,000-character one alone.
pub const CHARS_PER_BATCH_SLOT: usize = 256;

/// Group texts into batches that bound padding waste. The runtime pads every text in a batch to
/// the longest one, so a single long description in a batch of 64 short names costs like 64
/// long texts — measured on a mixed corpus as a 10× throughput loss. Texts are sorted by length
/// (longest first) and a batch closes when it reaches `batch_size` texts or when
/// `longest × count` would exceed `char_budget`. Returns index lists; callers restore order.
/// Vectors do not depend on batch composition (padding is masked), only speed does.
pub fn plan_batches(lengths: &[usize], batch_size: usize, char_budget: usize) -> Vec<Vec<usize>> {
    let batch_size = batch_size.max(1);
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lengths[i]));
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut longest = 0usize;
    for i in order {
        let len = lengths[i].max(1);
        let would_be_longest = longest.max(len);
        if !current.is_empty()
            && (current.len() >= batch_size || would_be_longest * (current.len() + 1) > char_budget)
        {
            batches.push(std::mem::take(&mut current));
            longest = 0;
        }
        longest = longest.max(len);
        current.push(i);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

#[cfg(test)]
mod batch_tests {
    use super::plan_batches;

    #[test]
    fn short_texts_fill_whole_batches_and_order_is_recoverable() {
        let lengths = vec![40; 150];
        let batches = plan_batches(&lengths, 64, 64 * 256);
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![64, 64, 22]
        );
        let mut all: Vec<usize> = batches.concat();
        all.sort_unstable();
        assert_eq!(all, (0..150).collect::<Vec<_>>());
    }

    #[test]
    fn long_texts_run_in_small_batches_and_short_ones_stay_together() {
        // one 10k text, three 2k texts, sixty 50-char names
        let mut lengths = vec![10_000, 2_000, 2_000, 2_000];
        lengths.extend(std::iter::repeat_n(50, 60));
        let batches = plan_batches(&lengths, 64, 64 * 256);
        assert_eq!(batches[0], vec![0]); // 10k alone: a second text would double the padding
        // The 2k texts open the next batch; at longest 2000 the budget admits 8 texts, so five
        // short names ride along, padded to 2000 — still 8× cheaper than padding 64 of them.
        assert_eq!(batches[1].len(), 8);
        assert!((1..=3).all(|i| batches[1].contains(&i)));
        assert_eq!(batches[2].len(), 55); // the remaining short names in one batch
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        assert!(plan_batches(&[], 64, 1).is_empty());
        assert_eq!(plan_batches(&[0, 0], 0, 0), vec![vec![0], vec![1]]);
    }
}
