//! PostgreSQL-backed IPFS cache implementation.
//!
//! Reads from the `ipfs_cache` table populated by `hermes-ipfs-cache`.
//!
//! Note: As of v2, the cache stores raw GRC2/GRC2Z payload bytes that have
//! been validated by hermes-ipfs-cache. No decoding is performed here.

use std::env;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::{Postgres, Row, postgres::PgPoolOptions};

use super::{CacheError, CachedEdit, IpfsCache};

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// PostgreSQL-backed IPFS cache.
///
/// Connects to the same database as `hermes-ipfs-cache` to read pre-fetched
/// and validated IPFS content. The cache is keyed by IPFS URI (e.g., "ipfs://Qm...").
///
/// The stored bytes are raw GRC2/GRC2Z format, already validated by hermes-ipfs-cache.
/// Consumers (e.g., kg-indexer) must decode using the grc-20 crate.
pub struct PostgresCache {
    pool: sqlx::Pool<Postgres>,
    /// Flag-off shadow read against the object-store copy
    /// (`IPFS_CACHE_BLOB_SHADOW_READ=1`); never changes what `get` returns.
    blob: Option<std::sync::Arc<blob_store::Mirror>>,
    /// Flag-off shadow of the warmer cursor read against geo-db (`GEODB_META_BLOCK_SHADOW=1`);
    /// never changes what `warmer_block` returns.
    meta_shadow: Option<geodb_cursor::meta_read::MetaBlockShadow>,
    /// Where `warmer_block` answers from (`WARMER_BLOCK_SOURCE`), default Postgres.
    warmer_source: WarmerSource,
}

/// Where the warmer's block is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmerSource {
    /// The `meta` row in Postgres (default).
    Postgres,
    /// geo-db's copy of the cursor (collection 10). No Postgres statement is sent. Needs the same
    /// reader configuration as `GEODB_META_BLOCK_SHADOW=1`.
    Geodb,
}

/// Parse `WARMER_BLOCK_SOURCE`: only `geodb` selects geo-db; unset, empty or anything else keeps
/// Postgres, so a typo never changes where the answer comes from.
pub fn warmer_source_from(value: Option<&str>) -> WarmerSource {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("geodb") => WarmerSource::Geodb,
        _ => WarmerSource::Postgres,
    }
}

impl PostgresCache {
    /// Create a new cache connected to the given database.
    pub async fn new(database_url: &str) -> Result<Self, CacheError> {
        let pool = PgPoolOptions::new()
            .max_connections(env_or("PG_POOL_MAX", 20))
            // Close idle connections after 30s to free PgBouncer slots.
            .idle_timeout(Duration::from_secs(env_or("PG_IDLE_TIMEOUT_SECS", 30)))
            // Fail fast when pool is saturated — 3s means all 20 connections are busy,
            // indicating DB trouble, not normal load (max observed query time ~6s).
            .acquire_timeout(Duration::from_secs(env_or("PG_ACQUIRE_TIMEOUT_SECS", 3)))
            .connect(database_url)
            .await
            .map_err(|e| CacheError::Database(e.to_string()))?;

        Ok(Self::from_parts(
            pool,
            blob_store::Mirror::from_env("ipfs_cache", "IPFS_CACHE")
                .filter(|m| m.flags().shadow_read)
                .map(std::sync::Arc::new),
            geodb_cursor::meta_read::MetaBlockShadow::from_env(),
            warmer_source_from(env::var("WARMER_BLOCK_SOURCE").ok().as_deref()),
        ))
    }

    /// Assemble from parts. Asking for the geo-db source with no reader configured logs an error
    /// and keeps Postgres, so the warmer never answers "unknown" forever.
    pub fn from_parts(
        pool: sqlx::Pool<Postgres>,
        blob: Option<std::sync::Arc<blob_store::Mirror>>,
        meta_shadow: Option<geodb_cursor::meta_read::MetaBlockShadow>,
        warmer_source: WarmerSource,
    ) -> Self {
        let warmer_source = if warmer_source == WarmerSource::Geodb && meta_shadow.is_none() {
            tracing::error!(
                "WARMER_BLOCK_SOURCE=geodb needs GEODB_META_BLOCK_SHADOW=1 and the geo-db key and \
                 binding paths; reading the warmer cursor from Postgres"
            );
            WarmerSource::Postgres
        } else {
            warmer_source
        };
        PostgresCache {
            pool,
            blob,
            meta_shadow,
            warmer_source,
        }
    }

    /// The source `warmer_block` actually uses.
    pub fn warmer_source(&self) -> WarmerSource {
        self.warmer_source
    }
}

/// `meta.block_number` is stored as text by every indexer that writes `meta`.
fn parse_block_number(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// `meta.id` under which hermes-ipfs-cache persists its cursor. Must match the
/// indexer id the warmer writes (see hermes-ipfs-cache's cursor store).
const IPFS_CACHE_INDEXER_ID: &str = "hermes_ipfs_cache";

#[async_trait]
impl IpfsCache for PostgresCache {
    /// Reads the warmer's durable cursor from the shared `meta` table. On any
    /// error this returns `None` — "unknown", which callers treat as "not yet
    /// processed" so a transient DB blip can never be mistaken for a verdict
    /// that content is unfetchable.
    ///
    /// `meta.block_number` is TEXT. This used to decode it as `i64`; the type
    /// mismatch was swallowed by `.ok()`, so from #845 until 2026-10-05 this always
    /// returned `None` and every unfetchable URI held the pipeline for the full
    /// `warmer_wait_max` backstop (a ~2h indexer stall that day). Failures are now
    /// logged, so a wrong answer here can never again look like a slow warmer.
    async fn warmer_block(&self) -> Option<u64> {
        if let (WarmerSource::Geodb, Some(shadow)) = (self.warmer_source, &self.meta_shadow) {
            // geo-db is the source: no Postgres read. A failed read is "unknown" (None), like
            // a Postgres error, never a stale answer.
            return match shadow.read_block(IPFS_CACHE_INDEXER_ID).await {
                Ok(block) => {
                    if block.is_none() {
                        tracing::warn!(
                            id = IPFS_CACHE_INDEXER_ID,
                            "geo-db has no IPFS warmer cursor row"
                        );
                    }
                    block
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Could not read the IPFS warmer cursor from geo-db");
                    None
                }
            };
        }
        let row = sqlx::query_scalar::<_, String>("SELECT block_number FROM meta WHERE id = $1")
            .bind(IPFS_CACHE_INDEXER_ID)
            .fetch_optional(&self.pool)
            .await;
        // Counted against geo-db's copy of the same cursor; the answer below stays Postgres's.
        if let (Some(shadow), Ok(found)) = (&self.meta_shadow, &row) {
            let pg = found.as_deref().and_then(parse_block_number);
            shadow
                .observe("hermes_pipeline_warmer_block", IPFS_CACHE_INDEXER_ID, pg)
                .await;
        }
        match row {
            Ok(Some(text)) => {
                let parsed = parse_block_number(&text);
                if parsed.is_none() {
                    tracing::warn!(value = %text, "IPFS warmer cursor block_number is not a block number");
                }
                parsed
            }
            Ok(None) => {
                tracing::warn!(
                    id = IPFS_CACHE_INDEXER_ID,
                    "IPFS warmer has no cursor row in meta"
                );
                None
            }
            Err(error) => {
                tracing::warn!(error = %error, "Could not read the IPFS warmer cursor");
                None
            }
        }
    }

    async fn get(&self, ipfs_hash: &str, _space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        let row = sqlx::query(
            "SELECT data, space, is_errored, name, block FROM ipfs_cache WHERE uri = $1",
        )
        .bind(ipfs_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CacheError::Database(e.to_string()))?;

        match row {
            Some(row) => {
                let is_errored: bool = row.get("is_errored");
                let db_space_id: sqlx::types::Uuid = row.get("space");
                let space_id_bytes = db_space_id.as_bytes().to_vec();

                if let Some(blob) = &self.blob {
                    let expected = blob_store::ipfs_object::encode(&blob_store::ipfs_object::Row {
                        block: row.get("block"),
                        space: db_space_id.simple().to_string(),
                        is_errored,
                        name: row.get("name"),
                        data: row.get("data"),
                    });
                    blob.shadow_read_detached(blob_store::ipfs_object::key(ipfs_hash), expected);
                }

                if is_errored {
                    return Ok(CachedEdit::errored(ipfs_hash.to_string(), space_id_bytes));
                }

                let data: Option<Vec<u8>> = row.get("data");
                let name: Option<String> = row.get("name");
                match data {
                    Some(payload) => {
                        // Return raw bytes - already validated by hermes-ipfs-cache
                        Ok(CachedEdit::success(
                            ipfs_hash.to_string(),
                            payload,
                            space_id_bytes,
                            name,
                        ))
                    }
                    None => {
                        // Entry exists but no data - treat as errored
                        Ok(CachedEdit::errored(ipfs_hash.to_string(), space_id_bytes))
                    }
                }
            }
            None => Err(CacheError::NotFound(ipfs_hash.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_text_block_numbers_meta_stores() {
        assert_eq!(parse_block_number("72441"), Some(72441));
        assert_eq!(parse_block_number(" 72441\n"), Some(72441));
        assert_eq!(parse_block_number(""), None);
        assert_eq!(parse_block_number("-1"), None);
        assert_eq!(parse_block_number("abc"), None);
    }

    /// The 2026-10-05 regression, against a real Postgres: `meta.block_number` is
    /// TEXT, and reading it must yield the warmer's block. Ignored by default because
    /// it needs a database:
    ///   DATABASE_URL=postgres://… cargo test -p hermes-pipeline -- --ignored warmer_block_reads
    #[tokio::test]
    #[ignore = "needs DATABASE_URL pointing at a scratch Postgres"]
    async fn warmer_block_reads_the_text_cursor_meta_stores() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let cache = PostgresCache::new(&url).await.expect("connect");
        sqlx::query("CREATE TABLE IF NOT EXISTS meta (id text PRIMARY KEY, cursor text NOT NULL, block_number text NOT NULL)")
            .execute(&cache.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meta (id, cursor, block_number) VALUES ($1, 'c', '72441') ON CONFLICT (id) DO UPDATE SET block_number = '72441'")
            .bind(IPFS_CACHE_INDEXER_ID)
            .execute(&cache.pool)
            .await
            .unwrap();
        assert_eq!(cache.warmer_block().await, Some(72441));
    }

    #[test]
    fn warmer_source_defaults_to_postgres() {
        assert_eq!(warmer_source_from(None), WarmerSource::Postgres);
        assert_eq!(warmer_source_from(Some("")), WarmerSource::Postgres);
        assert_eq!(warmer_source_from(Some("garbage")), WarmerSource::Postgres);
        assert_eq!(warmer_source_from(Some(" GeoDB ")), WarmerSource::Geodb);
        assert_eq!(warmer_source_from(Some("geodb")), WarmerSource::Geodb);
    }

    fn fake_shadow(script: &str) -> geodb_cursor::meta_read::MetaBlockShadow {
        use geodb_cursor::{Config, derived_read::DerivedReader};
        geodb_cursor::meta_read::MetaBlockShadow::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            10,
        )
    }

    /// Answers every `get` with the row `<block>:cursor`.
    fn geodb_with_block(block: u64) -> geodb_cursor::meta_read::MetaBlockShadow {
        let hex: String = format!("{block}:cursor")
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        let body = format!("{}\t1.0\t{hex}", "0".repeat(32));
        fake_shadow(&format!(
            "echo ready; while IFS= read -r l; do printf '+{}\\n{body}'; done",
            body.len()
        ))
    }

    /// The 2026-10-05 shape with the flag on: the Postgres `meta` row is frozen (here, not even
    /// reachable: a listener that counts TCP accepts, so zero accepts means zero statements) and the
    /// warmer still reports geo-db's block. With the default source the same cache goes to Postgres.
    #[tokio::test]
    async fn geodb_source_answers_from_geodb_and_sends_no_postgres_statement() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let seen = accepted.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(sock);
            }
        });
        let pool = || {
            PgPoolOptions::new()
                .acquire_timeout(Duration::from_secs(1))
                .connect_lazy(&format!("postgres://u:p@127.0.0.1:{port}/db"))
                .unwrap()
        };

        let cache = PostgresCache::from_parts(
            pool(),
            None,
            Some(geodb_with_block(900)),
            WarmerSource::Geodb,
        );
        assert_eq!(cache.warmer_block().await, Some(900));
        assert_eq!(
            accepted.load(Ordering::SeqCst),
            0,
            "a statement reached Postgres"
        );

        // A geo-db failure is "unknown", never a stale Postgres answer.
        let dead = PostgresCache::from_parts(
            pool(),
            None,
            Some(fake_shadow("exit 1")),
            WarmerSource::Geodb,
        );
        assert_eq!(dead.warmer_block().await, None);
        assert_eq!(accepted.load(Ordering::SeqCst), 0);

        // Control: the default source reads Postgres (and, unreachable there, answers None).
        let pg = PostgresCache::from_parts(
            pool(),
            None,
            Some(geodb_with_block(900)),
            WarmerSource::Postgres,
        );
        assert_eq!(pg.warmer_block().await, None);
        assert!(
            accepted.load(Ordering::SeqCst) > 0,
            "control saw no connection"
        );
    }

    /// Geodb requested with no reader configured falls back to Postgres rather than answering
    /// "unknown" forever.
    #[tokio::test]
    async fn geodb_source_without_a_reader_falls_back_to_postgres() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://u:p@127.0.0.1:1/db")
            .unwrap();
        let cache = PostgresCache::from_parts(pool, None, None, WarmerSource::Geodb);
        assert_eq!(cache.warmer_source(), WarmerSource::Postgres);
    }
}
