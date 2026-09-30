#![cfg(feature = "onnx")]
//! Golden-vector test against a real bundle. Skipped unless `EMBEDDING_TEST_BUNDLE` points at a
//! verified bundle directory for `bundles/bge-small-en-v1.5-q` (see embedding-service/README.md
//! for how to fetch it). The fixture holds vectors from Python fastembed on the same artifacts;
//! a conforming provider reproduces them to <= 1e-4 in cosine.

use std::path::PathBuf;

use embedding::{EmbeddingProvider, OnnxOptions, Purpose, bundle};
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    dimensions: usize,
    documents: Vec<Item>,
    queries: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    text: String,
    vector: Vec<f32>,
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();
    let na: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    dot / (na * nb)
}

#[test]
fn reproduces_python_fastembed_vectors() {
    let Ok(dir) = std::env::var("EMBEDDING_TEST_BUNDLE") else {
        eprintln!("EMBEDDING_TEST_BUNDLE not set; skipping golden test");
        return;
    };
    let fixture: Golden =
        serde_json::from_str(include_str!("fixtures/bge-small-en-v1.5-q.golden.json")).unwrap();
    let (verified, provider) =
        bundle::load(&PathBuf::from(dir), OnnxOptions::default()).expect("bundle loads");
    assert_eq!(verified.descriptor.dimensions, fixture.dimensions);

    for (purpose, items) in [
        (Purpose::Document, &fixture.documents),
        (Purpose::Query, &fixture.queries),
    ] {
        let texts: Vec<String> = items.iter().map(|i| i.text.clone()).collect();
        let got = provider.embed(&texts, purpose).expect("embeds");
        assert_eq!(got.len(), items.len());
        for (item, vector) in items.iter().zip(&got) {
            assert_eq!(vector.len(), fixture.dimensions);
            let norm: f64 = vector
                .iter()
                .map(|x| (*x as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                (norm - 1.0).abs() < 1e-3,
                "not normalized: |v| = {norm} for {:?}",
                item.text
            );
            let c = cosine(vector, &item.vector);
            assert!(
                1.0 - c <= 1e-4,
                "cosine {c} vs golden for {:?} ({purpose:?})",
                item.text
            );
        }
    }
}
