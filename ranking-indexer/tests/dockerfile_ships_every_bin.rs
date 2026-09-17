//! Every `[[bin]]` in Cargo.toml must be copied into the runtime image.
//!
//! The runtime stage copies binaries one COPY line at a time, so adding a `[[bin]]`
//! without adding its line produces an image that builds, pushes, deploys and passes
//! every check — and then dies at startup with
//! `exec: "<name>": executable file not found in $PATH`.
//!
//! That is not hypothetical. `feed_composition_sample` shipped exactly that way: green
//! CI, a successful build, a successful deploy, a CronJob visible in the cluster, and a
//! container that could never start. Nothing before this test looked at the two files
//! together.
//!
//! Deliberately a plain file comparison with no database and no network, so it actually
//! runs — unlike the e2e tests in this crate, which return early when their DB env var
//! is absent and report success for work they skipped.

use std::fs;
use std::path::Path;

#[test]
fn dockerfile_copies_every_declared_bin() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let dockerfile = fs::read_to_string(root.join("Dockerfile")).expect("read Dockerfile");

    // `name = "..."` lines that follow a `[[bin]]` header, up to the next section.
    let mut bins = Vec::new();
    let mut in_bin = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if in_bin {
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(v) = rest.split('=').nth(1) {
                    bins.push(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }

    assert!(
        !bins.is_empty(),
        "parsed no [[bin]] entries from Cargo.toml — the parser broke, not the Dockerfile"
    );

    let missing: Vec<&String> = bins
        .iter()
        .filter(|b| !dockerfile.contains(&format!("/usr/local/bin/{b}")))
        .collect();

    assert!(
        missing.is_empty(),
        "Cargo.toml declares {:?} but the Dockerfile never copies {:?} into the runtime \
         image. Such a binary deploys cleanly and then fails to start with \
         'executable file not found in $PATH'. Add a COPY line for each.",
        bins,
        missing
    );
}
