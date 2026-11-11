//! The geo-db side of `reconcile-orphans` (zero-Postgres plan 10.11, ORPHANS-RECONCILE-GEODB).
//!
//! # What an orphan is in geo-db terms
//!
//! Postgres answers "does this document have a graph behind it" with two probes: an `entities` row
//! for the id, and a `values` or outgoing `relations` row in the document's Space. geo-db has no
//! registry of bare ids: it holds an entity only as the subject of a live statement in a Space
//! (see plan 10.22 section 11.4). So the same two questions become one:
//!
//! - the **holders** of the entity: the Spaces where it is the subject of at least one active
//!   statement, from one `entity` read with `.args.holders = true` over every Space;
//! - **missing-entity**: the entity has no holder at all (no live statement in any Space, which is
//!   also what a Postgres id that was only ever named, never given a statement, looks like);
//! - **missing-in-space**: the entity has holders, but not the document's Space.
//!
//! The two definitions can disagree on purpose, and the compare mode exists to count how often:
//! Postgres `entities` keeps every id an edit ever named and is never pruned, so an id whose
//! statements were all deleted is *present* in Postgres and *absent* in geo-db; and derived rows
//! (vote scores, rank positions) are values in Postgres but not statements in geo-db.
//!
//! Ids are conformed first: geo-db holds a gaia id that is not a UUIDv4 under a one-way hash
//! (`api/src/geodb/card.ts` `conformingId`), for entities and for Spaces alike.
//!
//! # Transport
//!
//! A pool of long-lived `geodb-client pipe` children, the way the api reads cards: one request
//! line `*\t<geoql>\n` answers one frame, `+<n>\n<n bytes>` rows or `-<n>\n<n bytes>` a refusal.
//! A read only needs an authenticated session. Nothing is ever written to geo-db.

use std::collections::{BTreeMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use uuid::Uuid;

use super::reconcile_orphans::{DocRef, OrphanKind, classify};

/// Which side decides whether a document is an orphan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GeoDbMode {
    /// Postgres decides, as before. geo-db is never contacted.
    Off,
    /// Postgres decides and geo-db is asked too; the two verdicts are counted against each other.
    /// Report only: pruning is refused.
    Compare,
    /// geo-db decides and Postgres is not contacted. Report only: pruning is refused.
    Only,
}

/// How long one read may take before its worker is replaced.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest frame accepted: a holders answer is one row a Space.
const FRAME_MAX: usize = 4 << 20;

/// The id geo-db holds for a gaia id: a UUIDv4 passes through, any other is remapped by hash.
/// Mirrors `conformingId` in `api/src/geodb/card.ts` and `emit/lib.mjs`.
pub fn conforming_id(kind: &str, legacy: &Uuid) -> String {
    let id = legacy.simple().to_string();
    if legacy.get_version_num() == 4 && matches!(legacy.get_variant(), uuid::Variant::RFC4122) {
        return id;
    }
    let digest = Sha256::digest(format!("geo:legacy-{kind}:v1\0{id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The pipe request line for the Spaces holding one entity, every Space.
pub fn holders_line(entity: &str) -> String {
    format!(
        "*\t.{{ .version = 1, .select = .{{ .c = .{{ .field = \"entity\", .args = .{{ .id = \"{entity}\", .holders = true }} }} }} }}\n"
    )
}

/// The Spaces a holders answer names. Each row is tab-separated (property, value, statement,
/// entity): the value is the Space, the entity the one asked. A row that is anything else is an
/// error, never skipped, so an answer that is not a holders list is not read as an empty one.
pub fn parse_holders(body: &str, entity: &str) -> Result<HashSet<String>> {
    let mut spaces = HashSet::new();
    for line in body.lines().filter(|l| !l.is_empty()) {
        let cells: Vec<&str> = line.split('\t').collect();
        let space = cells.get(1).copied().unwrap_or("");
        if cells.get(3).copied() != Some(entity) || !is_hex32(space) {
            bail!(
                "a holders row is not a Space of the entity: {}",
                line.chars().take(60).collect::<String>()
            );
        }
        spaces.insert(space.to_string());
    }
    Ok(spaces)
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What geo-db says about one document, from the holders of its entity.
pub fn classify_holders(
    holders: &HashSet<String>,
    geo_space: &str,
    topic_stub: bool,
    soft_deleted: bool,
) -> Option<OrphanKind> {
    classify(
        !holders.is_empty(),
        holders.contains(geo_space),
        topic_stub,
        soft_deleted,
    )
}

/// Where and how to start a worker.
#[derive(Debug, Clone)]
pub struct GeoDbConfig {
    pub client_bin: String,
    pub key_path: String,
    pub binding_path: String,
    pub workers: usize,
}

struct Pipe {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Pipe {
    fn start(config: &GeoDbConfig) -> Result<Pipe> {
        let mut child = Command::new(&config.client_bin)
            .args([
                "--key",
                &config.key_path,
                "--binding",
                &config.binding_path,
                "pipe",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to start {}", config.client_bin))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        Ok(Pipe {
            child,
            stdin,
            stdout,
        })
    }

    /// One request line, one frame. A `-` frame is a refusal and an error here.
    async fn read(&mut self, line: &str) -> Result<String> {
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        let mut head = String::new();
        if self.stdout.read_line(&mut head).await? == 0 {
            bail!("geodb-client pipe closed");
        }
        let head = head.trim_end();
        let (sign, len) = head.split_at(head.len().min(1));
        let len: usize = len.parse().map_err(|_| {
            anyhow!(
                "bad frame header: {}",
                head.chars().take(40).collect::<String>()
            )
        })?;
        if len > FRAME_MAX {
            bail!("frame of {len} bytes exceeds {FRAME_MAX}");
        }
        let mut body = vec![0u8; len];
        self.stdout.read_exact(&mut body).await?;
        let body = String::from_utf8(body).context("frame is not utf-8")?;
        match sign {
            "+" => Ok(body),
            "-" => bail!(
                "geo-db refused: {}",
                body.chars().take(200).collect::<String>()
            ),
            _ => bail!(
                "bad frame header: {}",
                head.chars().take(40).collect::<String>()
            ),
        }
    }
}

/// A fixed set of workers; a worker that failed is replaced and its read retried once.
pub struct GeoDbChecker {
    config: GeoDbConfig,
    pipes: Vec<Arc<Mutex<Option<Pipe>>>>,
}

impl GeoDbChecker {
    pub fn new(config: GeoDbConfig) -> Self {
        let pipes = (0..config.workers.max(1))
            .map(|_| Arc::new(Mutex::new(None)))
            .collect();
        Self { config, pipes }
    }

    async fn holders(
        config: &GeoDbConfig,
        slot: &Mutex<Option<Pipe>>,
        entity: &str,
    ) -> Result<HashSet<String>> {
        let mut guard = slot.lock().await;
        let mut last = anyhow!("no attempt");
        for _ in 0..2 {
            if guard.is_none() {
                *guard = Some(Pipe::start(config)?);
            }
            let pipe = guard.as_mut().expect("started above");
            match tokio::time::timeout(READ_TIMEOUT, pipe.read(&holders_line(entity))).await {
                Ok(Ok(body)) => return parse_holders(&body, entity),
                Ok(Err(e)) => last = e,
                Err(_) => last = anyhow!("read timed out after {READ_TIMEOUT:?}"),
            }
            // A worker that failed or timed out cannot be trusted with a reply still owed.
            if let Some(mut dead) = guard.take() {
                let _ = dead.child.start_kill();
            }
        }
        Err(last.context(format!("geo-db holders read failed for entity {entity}")))
    }

    /// The orphans among `docs` by geo-db's account: one holders read per distinct entity,
    /// spread over the workers.
    pub async fn check_page(&self, docs: &[DocRef]) -> Result<Vec<(DocRef, OrphanKind)>> {
        let mut by_entity: BTreeMap<String, Vec<&DocRef>> = BTreeMap::new();
        for doc in docs {
            by_entity
                .entry(conforming_id("entity", &doc.entity_id))
                .or_default()
                .push(doc);
        }
        let entities: Vec<String> = by_entity.keys().cloned().collect();
        let mut tasks = JoinSet::new();
        for (w, slot) in self.pipes.iter().enumerate() {
            let mine: Vec<String> = entities
                .iter()
                .skip(w)
                .step_by(self.pipes.len())
                .cloned()
                .collect();
            let slot = Arc::clone(slot);
            let config = self.config.clone();
            tasks.spawn(async move {
                let mut out = Vec::with_capacity(mine.len());
                for entity in mine {
                    let holders = Self::holders(&config, &slot, &entity).await?;
                    out.push((entity, holders));
                }
                Ok::<_, anyhow::Error>(out)
            });
        }
        let mut found: Vec<(DocRef, OrphanKind)> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            for (entity, holders) in joined.context("holders task panicked")?? {
                for doc in &by_entity[&entity] {
                    let space = conforming_id("space", &doc.space_id);
                    if let Some(kind) =
                        classify_holders(&holders, &space, doc.topic_stub, doc.soft_deleted)
                    {
                        found.push(((*doc).clone(), kind));
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.doc_id.cmp(&b.0.doc_id));
        Ok(found)
    }
}

/// How the two verdicts on one set of documents line up.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Compare {
    /// Both sides call it an orphan, of the same kind.
    pub both_same_kind: usize,
    /// Both call it an orphan, of different kinds.
    pub both_other_kind: usize,
    /// Postgres calls it an orphan and geo-db holds it.
    pub postgres_only: usize,
    /// geo-db calls it an orphan and Postgres holds it.
    pub geodb_only: usize,
}

impl Compare {
    pub fn disagreements(&self) -> usize {
        self.both_other_kind + self.postgres_only + self.geodb_only
    }
}

/// Count the two verdicts against each other. Both lists are orphans by doc id.
pub fn compare(
    postgres: &[(DocRef, OrphanKind)],
    geodb: &[(DocRef, OrphanKind)],
) -> (Compare, Vec<(String, &'static str)>) {
    let pg: BTreeMap<&str, OrphanKind> = postgres
        .iter()
        .map(|(d, k)| (d.doc_id.as_str(), *k))
        .collect();
    let geo: BTreeMap<&str, OrphanKind> =
        geodb.iter().map(|(d, k)| (d.doc_id.as_str(), *k)).collect();
    let mut tally = Compare::default();
    let mut samples = Vec::new();
    for (id, kind) in &pg {
        match geo.get(id) {
            Some(other) if other == kind => tally.both_same_kind += 1,
            Some(_) => {
                tally.both_other_kind += 1;
                samples.push((id.to_string(), "kind_differs"));
            }
            None => {
                tally.postgres_only += 1;
                samples.push((id.to_string(), "postgres_only"));
            }
        }
    }
    for id in geo.keys().filter(|id| !pg.contains_key(*id)) {
        tally.geodb_only += 1;
        samples.push((id.to_string(), "geodb_only"));
    }
    (tally, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: &str = "13711444-bd8d-405f-9985-2b523d112166";

    fn doc(id: &str, entity: &str, space: &str) -> DocRef {
        DocRef {
            doc_id: id.into(),
            entity_id: Uuid::parse_str(entity).unwrap(),
            space_id: Uuid::parse_str(space).unwrap(),
            topic_stub: false,
            soft_deleted: false,
            name: None,
        }
    }

    #[test]
    fn a_v4_id_conforms_to_itself() {
        let id = Uuid::parse_str(E).unwrap();
        assert_eq!(
            conforming_id("entity", &id),
            "13711444bd8d405f99852b523d112166"
        );
        assert_eq!(
            conforming_id("space", &id),
            "13711444bd8d405f99852b523d112166"
        );
    }

    #[test]
    fn a_legacy_id_conforms_by_hash_as_the_api_does() {
        // Vectors from api/src/geodb/card.ts conformingId (node, 2026-11-09).
        let v7ish = Uuid::parse_str("c9f267dc-b0d2-7071-8c2a-3c45a64afd32").unwrap();
        assert_eq!(
            conforming_id("entity", &v7ish),
            "1f2e4266b8e74aff91bfc3e122e0dd81"
        );
        assert_eq!(
            conforming_id("space", &v7ish),
            "29b102a14686469c87e6a726f4ab9698"
        );
        // The api's own test vector.
        let one = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(
            conforming_id("space", &one),
            "e2c3812c809a4b56bd7bf07b987af9b2"
        );
    }

    #[test]
    fn holders_rows_name_the_spaces_and_refuse_anything_else() {
        let e = "13711444bd8d405f99852b523d112166";
        let s = "c9f267dcb0d270718c2a3c45a64afd32";
        let body = format!("none\t{s}\tnone\t{e}\n");
        let got = parse_holders(&body, e).unwrap();
        assert_eq!(got, HashSet::from([s.to_string()]));
        assert!(parse_holders("", e).unwrap().is_empty());
        assert!(parse_holders(&format!("none\t{s}\tnone\t{}\n", "0".repeat(32)), e).is_err());
        assert!(
            parse_holders(
                "none\tnot-a-space\tnone\t13711444bd8d405f99852b523d112166\n",
                e
            )
            .is_err()
        );
    }

    #[test]
    fn holders_classify_as_missing_entity_missing_in_space_or_present() {
        let held = HashSet::from(["aa".repeat(16)]);
        assert_eq!(
            classify_holders(&held, &"aa".repeat(16), false, false),
            None
        );
        assert_eq!(
            classify_holders(&held, &"bb".repeat(16), false, false),
            Some(OrphanKind::MissingInSpace)
        );
        assert_eq!(
            classify_holders(&HashSet::new(), &"aa".repeat(16), false, false),
            Some(OrphanKind::MissingEntity)
        );
        // Tombstones and topic stubs are never orphans, as on the Postgres side.
        assert_eq!(classify_holders(&HashSet::new(), "x", true, false), None);
        assert_eq!(classify_holders(&HashSet::new(), "x", false, true), None);
    }

    #[test]
    fn the_request_line_names_every_space_and_one_entity() {
        let line = holders_line("13711444bd8d405f99852b523d112166");
        assert!(line.starts_with("*\t.{ .version = 1,"));
        assert!(line.contains(".holders = true"));
        assert!(line.ends_with("}\n"));
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn compare_counts_each_way_the_two_sides_can_differ() {
        let s = "c9f267dc-b0d2-7071-8c2a-3c45a64afd32";
        let a = doc("a", E, s);
        let b = doc("b", E, s);
        let c = doc("c", E, s);
        let d = doc("d", E, s);
        let pg = vec![
            (a.clone(), OrphanKind::MissingEntity),
            (b.clone(), OrphanKind::MissingEntity),
            (c.clone(), OrphanKind::MissingInSpace),
        ];
        let geo = vec![
            (a, OrphanKind::MissingEntity),
            (b, OrphanKind::MissingInSpace),
            (d, OrphanKind::MissingInSpace),
        ];
        let (tally, samples) = compare(&pg, &geo);
        assert_eq!(
            tally,
            Compare {
                both_same_kind: 1,
                both_other_kind: 1,
                postgres_only: 1,
                geodb_only: 1
            }
        );
        assert_eq!(tally.disagreements(), 3);
        assert_eq!(samples.len(), 3);
    }

    /// A stand-in for `geodb-client pipe`: answers each request line with a holders frame naming
    /// one Space for the entity in the line, or a refusal frame for the entity ending in `0`.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_pipe_reads_frames_and_refuses_on_a_minus_frame() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("geodb-check-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-geodb-client");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
while IFS= read -r line; do
  e=$(printf '%s' "$line" | sed 's/.*\.id = "\([0-9a-f]*\)".*/\1/')
  case "$e" in
    *0) printf -- '-7\nrefused' ;;
    *) row=$(printf 'none\t%s\tnone\t%s\n' cccccccccccc4ccc8ccccccccccccccc "$e"); printf '+%s\n%s' "${#row}" "$row" ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = GeoDbConfig {
            client_bin: bin.to_string_lossy().into(),
            key_path: "k".into(),
            binding_path: "b".into(),
            workers: 2,
        };
        let checker = GeoDbChecker::new(config.clone());
        let held = "13711444-bd8d-405f-9985-2b523d112161";
        let held_space = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
        let other_space = "c9f267dc-b0d2-4071-8c2a-3c45a64afd32";
        let docs = vec![
            doc("in", held, held_space),
            doc("elsewhere", held, other_space),
        ];
        let found = checker.check_page(&docs).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.doc_id, "elsewhere");
        assert_eq!(found[0].1, OrphanKind::MissingInSpace);

        // A refusal is an error naming the entity, after the one retry.
        let refused = vec![doc("r", "13711444-bd8d-405f-9985-2b523d112160", held_space)];
        let err = checker.check_page(&refused).await.unwrap_err();
        assert!(format!("{err:#}").contains("refused"), "{err:#}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
