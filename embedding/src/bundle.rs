//! Bundles: a directory holding `bundle.json` (the descriptor) and the artifacts it pins.
//!
//! Layout: `<models_dir>/<slot>/{bundle.json, model.onnx, tokenizer.json, ...}`. The directory
//! name is the slot id derived from `bundle.json`, so an image tag or a `ls /models` tells you
//! which slots a process serves. Loading refuses any file whose SHA-256 differs from the
//! descriptor, and any directory whose name does not match its own descriptor.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::descriptor::{Descriptor, TOKENIZER_FILES, is_slot_id};
use crate::error::{Error, Result};
use crate::onnx::{BundleFiles, OnnxLocalProvider, OnnxOptions};

pub const BUNDLE_FILE: &str = "bundle.json";

#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    pub dir: PathBuf,
    pub descriptor: Descriptor,
    pub slot: String,
    pub hash: String,
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub fn read_descriptor(dir: &Path) -> Result<Descriptor> {
    let path = dir.join(BUNDLE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| io(&path, e))?;
    let descriptor: Descriptor = serde_json::from_str(&text)?;
    descriptor.validate()?;
    Ok(descriptor)
}

/// SHA-256 of a file as `sha256:<hex>`, streamed.
pub fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|e| io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Read the descriptor, hash every artifact against it, and check the directory name.
pub fn verify(dir: &Path) -> Result<VerifiedBundle> {
    let descriptor = read_descriptor(dir)?;
    for (file, expected) in &descriptor.artifacts {
        let path = dir.join(file);
        if !path.is_file() {
            return Err(Error::Bundle {
                dir: dir.to_path_buf(),
                reason: format!("artifact {file} is missing"),
            });
        }
        let actual = digest_file(&path)?;
        if &actual != expected {
            return Err(Error::HashMismatch {
                dir: dir.to_path_buf(),
                file: file.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    let slot = descriptor.slot_id();
    if let Some(name) = dir.file_name().and_then(|n| n.to_str())
        && is_slot_id(name)
        && name != slot
    {
        return Err(Error::SlotMismatch {
            dir: dir.to_path_buf(),
            computed: slot,
            dir_name: name.to_string(),
        });
    }
    Ok(VerifiedBundle {
        dir: dir.to_path_buf(),
        hash: descriptor.hash(),
        slot,
        descriptor,
    })
}

fn read_artifact(dir: &Path, file: &str) -> Result<Vec<u8>> {
    let path = dir.join(file);
    fs::read(&path).map_err(|e| io(&path, e))
}

/// Verify a bundle and build its provider.
pub fn load(dir: &Path, options: OnnxOptions) -> Result<(VerifiedBundle, OnnxLocalProvider)> {
    let verified = verify(dir)?;
    let d = &verified.descriptor;
    let files = BundleFiles {
        model: read_artifact(dir, &d.model_file)?,
        tokenizer: read_artifact(dir, TOKENIZER_FILES[0])?,
        config: read_artifact(dir, TOKENIZER_FILES[1])?,
        special_tokens_map: read_artifact(dir, TOKENIZER_FILES[2])?,
        tokenizer_config: read_artifact(dir, TOKENIZER_FILES[3])?,
    };
    let provider = OnnxLocalProvider::new(d.clone(), files, options)?;
    Ok((verified, provider))
}

/// Every subdirectory of `models_dir` that holds a `bundle.json`, sorted by name.
pub fn discover(models_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(models_dir).map_err(|e| io(models_dir, e))?;
    for entry in entries {
        let path = entry.map_err(|e| io(models_dir, e))?.path();
        if path.is_dir() && path.join(BUNDLE_FILE).is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// One artifact to download: where from, and the digest it must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSource {
    pub file: String,
    pub url: String,
    pub expected: String,
}

/// Resolve `source` (`hf:<repo>@<revision>`) into one URL per artifact.
pub fn artifact_sources(descriptor: &Descriptor) -> Result<Vec<ArtifactSource>> {
    let unsupported = || Error::UnsupportedSource(descriptor.source.clone());
    let rest = descriptor
        .source
        .strip_prefix("hf:")
        .ok_or_else(unsupported)?;
    let (repo, revision) = rest.split_once('@').ok_or_else(unsupported)?;
    if repo.is_empty() || revision.is_empty() || !repo.contains('/') {
        return Err(unsupported());
    }
    Ok(descriptor
        .artifacts
        .iter()
        .map(|(file, expected)| ArtifactSource {
            file: file.clone(),
            url: format!("https://huggingface.co/{repo}/resolve/{revision}/{file}"),
            expected: expected.clone(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::test_support::sample;

    /// Write a bundle whose artifacts are small fake files, with digests computed from them.
    fn write_bundle(root: &Path, tamper: bool) -> (PathBuf, Descriptor) {
        let mut d = sample();
        let tmp = root.join("staging");
        fs::create_dir_all(&tmp).unwrap();
        for file in d.artifacts.clone().keys() {
            fs::write(tmp.join(file), format!("content of {file}")).unwrap();
            let digest = digest_file(&tmp.join(file)).unwrap();
            d.artifacts.insert(file.clone(), digest);
        }
        let dir = root.join(d.slot_id());
        fs::rename(&tmp, &dir).unwrap();
        fs::write(
            dir.join(BUNDLE_FILE),
            serde_json::to_string_pretty(&d).unwrap(),
        )
        .unwrap();
        if tamper {
            fs::write(dir.join("config.json"), "tampered").unwrap();
        }
        (dir, d)
    }

    #[test]
    fn verify_accepts_matching_hashes_and_directory_name() {
        let root = tempfile::tempdir().unwrap();
        let (dir, d) = write_bundle(root.path(), false);
        let v = verify(&dir).unwrap();
        assert_eq!(v.slot, d.slot_id());
        assert_eq!(v.hash, d.hash());
        assert_eq!(discover(root.path()).unwrap(), vec![dir]);
    }

    #[test]
    fn verify_refuses_a_tampered_artifact() {
        let root = tempfile::tempdir().unwrap();
        let (dir, _) = write_bundle(root.path(), true);
        assert!(
            matches!(verify(&dir), Err(Error::HashMismatch { file, .. }) if file == "config.json")
        );
    }

    #[test]
    fn verify_refuses_a_missing_artifact_and_a_wrong_directory_name() {
        let root = tempfile::tempdir().unwrap();
        let (dir, _) = write_bundle(root.path(), false);
        fs::remove_file(dir.join("tokenizer_config.json")).unwrap();
        assert!(matches!(verify(&dir), Err(Error::Bundle { .. })));

        let (dir, _) = write_bundle(&root.path().join("b"), false);
        let renamed = root.path().join("b").join("0123456789");
        fs::rename(&dir, &renamed).unwrap();
        assert!(matches!(verify(&renamed), Err(Error::SlotMismatch { .. })));
    }

    #[test]
    fn artifact_sources_resolve_hf_urls() {
        let d = sample();
        let sources = artifact_sources(&d).unwrap();
        assert_eq!(sources.len(), d.artifacts.len());
        let model = sources.iter().find(|s| s.file == "model.onnx").unwrap();
        assert_eq!(
            model.url,
            "https://huggingface.co/test/model/resolve/0123456789abcdef0123456789abcdef01234567/model.onnx"
        );
        let mut bad = d.clone();
        bad.source = "s3://bucket/x".into();
        assert!(matches!(
            artifact_sources(&bad),
            Err(Error::UnsupportedSource(_))
        ));
    }
}
