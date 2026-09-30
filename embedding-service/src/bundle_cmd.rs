//! `bundle` subcommands: fetch artifacts for a committed descriptor, verify a bundle directory,
//! or print the slot id a descriptor hashes to. `fetch` is what the Dockerfile runs, so the image
//! never contains a byte the descriptor did not pin.

use std::path::Path;

use anyhow::{Context, bail};
use embedding::bundle::{self, BUNDLE_FILE, artifact_sources, digest_file};
use tracing::info;

pub fn slot(spec: &Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(spec).with_context(|| format!("reading {}", spec.display()))?;
    let descriptor: embedding::Descriptor = serde_json::from_str(&text)?;
    descriptor.validate()?;
    println!("{}", descriptor.slot_id());
    Ok(())
}

pub fn verify(dir: &Path) -> anyhow::Result<()> {
    let v = bundle::verify(dir)?;
    println!(
        "slot={} hash={} model_id={} dimensions={}",
        v.slot, v.hash, v.descriptor.model_id, v.descriptor.dimensions
    );
    Ok(())
}

/// Download every artifact of `spec` into `<out>/<slot>/`, verify, and place the descriptor.
/// Idempotent: artifacts already present with the right digest are not downloaded again.
pub async fn fetch(spec: &Path, out: &Path) -> anyhow::Result<()> {
    let text =
        std::fs::read_to_string(spec).with_context(|| format!("reading {}", spec.display()))?;
    let descriptor: embedding::Descriptor = serde_json::from_str(&text)?;
    descriptor.validate()?;
    let slot = descriptor.slot_id();
    let dir = out.join(&slot);
    std::fs::create_dir_all(&dir)?;

    let client = reqwest::Client::builder()
        .user_agent("gaia-embedding-service/bundle-fetch")
        .build()?;
    for source in artifact_sources(&descriptor)? {
        let target = dir.join(&source.file);
        if target.is_file() && digest_file(&target)? == source.expected {
            info!(file = %source.file, "present and verified, skipping download");
            continue;
        }
        info!(file = %source.file, url = %source.url, "downloading");
        let response = client.get(&source.url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        let tmp = dir.join(format!("{}.part", source.file));
        std::fs::write(&tmp, &bytes)?;
        let actual = digest_file(&tmp)?;
        if actual != source.expected {
            let _ = std::fs::remove_file(&tmp);
            bail!(
                "{}: downloaded digest {} does not match descriptor {}",
                source.file,
                actual,
                source.expected
            );
        }
        std::fs::rename(&tmp, &target)?;
    }
    std::fs::write(
        dir.join(BUNDLE_FILE),
        serde_json::to_string_pretty(&descriptor)? + "\n",
    )?;
    let v = bundle::verify(&dir)?;
    println!("slot={} hash={} dir={}", v.slot, v.hash, dir.display());
    Ok(())
}
