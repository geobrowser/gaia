//! IPFS client for fetching GRC-20 edit content.
//!
//! This crate provides:
//! - [`IpfsSource`] config enum for choosing between mock and live IPFS clients
//! - [`IpfsFetcher`] trait for abstracting IPFS access
//! - [`IpfsClient`] production client that fetches from an IPFS gateway
//! - [`MockIpfsClient`] mock client for testing with pre-configured CID → bytes mappings
//!
//! ## Usage with IpfsSource (Recommended)
//!
//! ```ignore
//! use ipfs::IpfsSource;
//! use std::collections::HashMap;
//!
//! // Development/testing: use mock data
//! let mut data = HashMap::new();
//! data.insert("QmTestCid1".to_string(), grc20_bytes);
//! let fetcher = IpfsSource::mock_bytes(data).into_fetcher();
//!
//! // Production: use live gateway
//! let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
//!
//! // Use the fetcher
//! let bytes = fetcher.get_bytes("ipfs://QmTestCid1").await?;
//! ```

mod mock;
mod verify;

pub use mock::MockIpfsClient;
pub use verify::Verification;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client as ReqwestClient;
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum IpfsError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("network error: {0}")]
    NetworkError(String),
    #[error("timeout")]
    Timeout,
    #[error("content did not hash to the requested CID after {attempts} attempt(s): {cid}")]
    VerificationFailed { cid: String, attempts: u32 },
}

pub type Result<T> = std::result::Result<T, IpfsError>;

/// Trait for fetching content from IPFS.
///
/// This trait abstracts the IPFS client to enable dependency injection
/// and mocking for testing. Production code uses [`IpfsClient`], while
/// tests can use mock implementations.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsFetcher;
///
/// async fn fetch_bytes<F: IpfsFetcher>(fetcher: &F, uri: &str) -> Result<Vec<u8>> {
///     fetcher.get_bytes(uri).await
/// }
/// ```
#[async_trait]
pub trait IpfsFetcher: Send + Sync {
    /// Fetch raw bytes from IPFS by CID.
    ///
    /// The URI should be in the format `ipfs://CID` or just a raw CID.
    async fn get_bytes(&self, cid: &str) -> Result<Vec<u8>>;
}

/// Production IPFS client that fetches from a gateway.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsClient;
///
/// let client = IpfsClient::new("https://ipfs.io/ipfs/");
/// let bytes = client.get_bytes("ipfs://QmYwAPJzv5CZsnA...").await?;
/// ```
pub struct IpfsClient {
    url: String,
    client: ReqwestClient,
    max_attempts: u32,
    base_backoff: Duration,
}

impl IpfsClient {
    /// Max fetch attempts before giving up on a URI. Applies to both
    /// transport failures (timeouts, non-2xx) and content that fails CID
    /// verification (see [`Verification`]) — either is treated as "this
    /// attempt didn't get the real content," not "this content is invalid."
    const DEFAULT_MAX_ATTEMPTS: u32 = 3;
    const DEFAULT_BASE_BACKOFF: Duration = Duration::from_millis(250);
    /// Upper bound on any single retry's backoff, regardless of attempt
    /// count or configured base — keeps a large `max_attempts` from ever
    /// producing an absurd (if not overflowing) wait.
    const MAX_BACKOFF: Duration = Duration::from_secs(30);

    pub fn new(url: &str) -> Self {
        IpfsClient {
            url: url.to_string(),
            client: ReqwestClient::new(),
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
            base_backoff: Self::DEFAULT_BASE_BACKOFF,
        }
    }

    /// Same as [`Self::new`] but with a configurable attempt count and base
    /// backoff (doubled each retry). Exposed mainly so tests can run a full
    /// retry sequence without real delays.
    pub fn with_retry_config(url: &str, max_attempts: u32, base_backoff: Duration) -> Self {
        IpfsClient {
            url: url.to_string(),
            client: ReqwestClient::new(),
            max_attempts: max_attempts.max(1),
            base_backoff,
        }
    }

    async fn fetch_once(&self, cid: &str) -> Result<Vec<u8>> {
        // Normalize exactly one separating slash regardless of whether the
        // configured gateway URL ends with one, so a trailing-slash typo in
        // config doesn't turn into a malformed `https://hostQm...` URL. This
        // does NOT guess at a gateway's path convention (e.g. `/ipfs/` vs
        // `/files/`) — the configured URL must already include whatever
        // path segment that specific gateway needs; see
        // `hermes-ipfs-cache/k8s/production/secrets.yaml.template`.
        let base = self.url.trim_end_matches('/');
        let url = format!("{base}/{cid}");
        let res = self.client.get(&url).send().await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(IpfsError::NetworkError(format!(
                "HTTP {}: {}",
                status,
                body.trim()
            )));
        }

        let bytes = res.bytes().await?;
        Ok(bytes.to_vec())
    }
}

#[async_trait]
impl IpfsFetcher for IpfsClient {
    /// Fetch `uri`, retrying transient failures and re-fetching (rather than
    /// trusting) content whose hash doesn't match the requested CID.
    ///
    /// Without this, a single truncated/corrupted gateway response — still
    /// HTTP 200 — used to be indistinguishable from genuinely invalid
    /// content, and got cached as permanently errored with no retry
    /// anywhere upstream (`hermes-ipfs-cache` → `hermes-pipeline` silently
    /// dropping the edit for good). See the "Encoding error" incidents this
    /// was built to stop recurring.
    async fn get_bytes(&self, uri: &str) -> Result<Vec<u8>> {
        let cid = uri.strip_prefix("ipfs://").unwrap_or(uri);

        // Tracks whatever happened on the most recent attempt, so the final
        // error accurately reflects that attempt's actual failure mode
        // (transport error vs. hash mismatch) rather than conflating the
        // two across a mixed sequence of retries.
        let mut last_error: Option<IpfsError> = None;

        for attempt in 1..=self.max_attempts {
            match self.fetch_once(cid).await {
                Ok(bytes) => match verify::verify(cid, &bytes) {
                    Verification::Verified | Verification::Unsupported => return Ok(bytes),
                    Verification::Mismatch => {
                        warn!(
                            cid = %cid,
                            attempt,
                            max_attempts = self.max_attempts,
                            bytes = bytes.len(),
                            "fetched bytes did not hash to the requested CID, retrying"
                        );
                        last_error = Some(IpfsError::VerificationFailed {
                            cid: cid.to_string(),
                            attempts: attempt,
                        });
                    }
                },
                Err(err) => {
                    warn!(
                        cid = %cid,
                        attempt,
                        max_attempts = self.max_attempts,
                        error = %err,
                        "IPFS fetch failed, retrying"
                    );
                    last_error = Some(err);
                }
            }

            if attempt < self.max_attempts {
                // `checked_pow`/`saturating_mul` (rather than plain `*`) so a
                // large `max_attempts` via `with_retry_config` can't overflow
                // `u32`/`Duration` arithmetic; `.min(MAX_BACKOFF)` caps the
                // wait at something sane regardless.
                let multiplier = 2u32.checked_pow(attempt - 1).unwrap_or(u32::MAX);
                let backoff = self
                    .base_backoff
                    .saturating_mul(multiplier)
                    .min(Self::MAX_BACKOFF);
                tokio::time::sleep(backoff).await;
            }
        }

        // Every loop iteration sets last_error before falling through to
        // here (the only early return is the Ok(bytes) success path above),
        // so this is always Some by the time max_attempts is exhausted.
        Err(last_error
            .expect("get_bytes retry loop always records an error before exhausting attempts"))
    }
}

/// Configuration for the IPFS data source.
///
/// Use this to explicitly choose between mock and live IPFS clients,
/// following the same pattern as `StreamSource` in hermes-relay.
///
/// # Example
///
/// ```ignore
/// use ipfs::IpfsSource;
/// use std::collections::HashMap;
///
/// // Development/testing with GRC-20 v2 bytes (recommended)
/// let mut data = HashMap::new();
/// data.insert("QmTestCid1".to_string(), grc20_bytes);
/// let fetcher = IpfsSource::mock_bytes(data).into_fetcher();
///
/// // Production: use live gateway
/// let fetcher = IpfsSource::live("https://ipfs.io/ipfs/").into_fetcher();
/// ```
#[derive(Debug, Clone)]
pub enum IpfsSource {
    /// Use mock IPFS client with pre-configured CID → raw bytes mappings.
    ///
    /// Use this with GRC-20 v2 encoded bytes (`grc_20::encode_edit`).
    /// The map keys are CIDs (with or without `ipfs://` prefix).
    MockBytes(HashMap<String, Vec<u8>>),

    /// Connect to a live IPFS gateway.
    Live {
        /// The IPFS gateway URL (e.g., "https://ipfs.io/ipfs/")
        gateway_url: String,
    },
}

impl IpfsSource {
    /// Create a mock IPFS source with the given CID → raw bytes mappings.
    ///
    /// Use this with GRC-20 v2 encoded bytes (`grc_20::encode_edit`).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use grc_20::{encode_edit, Edit};
    /// use std::borrow::Cow;
    ///
    /// let edit = Edit {
    ///     id: [0x01; 16],
    ///     name: Cow::Borrowed("Test Edit"),
    ///     authors: vec![],
    ///     created_at: 1700000000,
    ///     ops: vec![],
    /// };
    /// let bytes = encode_edit(&edit).unwrap();
    ///
    /// let mut data = HashMap::new();
    /// data.insert("QmTestCid1".to_string(), bytes);
    /// let source = IpfsSource::mock_bytes(data);
    /// ```
    pub fn mock_bytes(data: HashMap<String, Vec<u8>>) -> Self {
        Self::MockBytes(data)
    }

    /// Create a live IPFS source with the given gateway URL.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let source = IpfsSource::live("https://ipfs.io/ipfs/");
    /// ```
    pub fn live(gateway_url: impl Into<String>) -> Self {
        Self::Live {
            gateway_url: gateway_url.into(),
        }
    }

    /// Create the appropriate IpfsFetcher implementation.
    ///
    /// Returns a boxed trait object that can be used to fetch IPFS content.
    pub fn into_fetcher(self) -> Box<dyn IpfsFetcher> {
        match self {
            Self::MockBytes(data) => Box::new(MockIpfsClient::with_bytes(data)),
            Self::Live { gateway_url } => Box::new(IpfsClient::new(&gateway_url)),
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use crate::verify::fixtures::{GOLDEN_SMALL_BYTES, GOLDEN_SMALL_CID};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn test_client(server: &MockServer) -> IpfsClient {
        IpfsClient::with_retry_config(&format!("{}/", server.uri()), 3, Duration::from_millis(1))
    }

    /// Fails with a 500 for the first `fail_times` requests, then returns
    /// `good_body` on every request after that.
    struct FlakyThenGood {
        fail_times: u32,
        calls: AtomicU32,
        good_body: Vec<u8>,
    }

    impl Respond for FlakyThenGood {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_times {
                ResponseTemplate::new(500)
            } else {
                ResponseTemplate::new(200).set_body_bytes(self.good_body.clone())
            }
        }
    }

    /// Always returns 200 with `body`, regardless of what's requested —
    /// used to simulate a gateway that returns wrong/corrupted content for
    /// a CID it claims to be serving.
    struct AlwaysBody {
        calls: Arc<AtomicU32>,
        body: Vec<u8>,
    }

    impl Respond for AlwaysBody {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_bytes(self.body.clone())
        }
    }

    #[tokio::test]
    async fn succeeds_when_base_url_has_no_trailing_slash() {
        // A trailing-slash typo in config shouldn't produce a malformed URL.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{GOLDEN_SMALL_CID}")))
            .respond_with(AlwaysBody {
                calls: Arc::new(AtomicU32::new(0)),
                body: GOLDEN_SMALL_BYTES.to_vec(),
            })
            .mount(&server)
            .await;

        let no_trailing_slash = server.uri();
        assert!(!no_trailing_slash.ends_with('/'));
        let client = IpfsClient::with_retry_config(&no_trailing_slash, 3, Duration::from_millis(1));

        let bytes = client.get_bytes(GOLDEN_SMALL_CID).await.unwrap();
        assert_eq!(bytes, GOLDEN_SMALL_BYTES);
    }

    #[tokio::test]
    async fn succeeds_immediately_on_valid_content() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicU32::new(0));
        Mock::given(method("GET"))
            .and(path(format!("/{GOLDEN_SMALL_CID}")))
            .respond_with(AlwaysBody {
                calls: Arc::clone(&calls),
                body: GOLDEN_SMALL_BYTES.to_vec(),
            })
            .mount(&server)
            .await;

        let client = test_client(&server);
        let bytes = client.get_bytes(GOLDEN_SMALL_CID).await.unwrap();

        assert_eq!(bytes, GOLDEN_SMALL_BYTES);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "should not retry on success"
        );
    }

    #[tokio::test]
    async fn retries_transient_http_failures_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/{GOLDEN_SMALL_CID}")))
            .respond_with(FlakyThenGood {
                fail_times: 2,
                calls: AtomicU32::new(0),
                good_body: GOLDEN_SMALL_BYTES.to_vec(),
            })
            .mount(&server)
            .await;

        let client = test_client(&server);
        let bytes = client.get_bytes(GOLDEN_SMALL_CID).await.unwrap();

        assert_eq!(bytes, GOLDEN_SMALL_BYTES);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_on_persistent_http_failure() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicU32::new(0));
        Mock::given(method("GET"))
            .and(path(format!("/{GOLDEN_SMALL_CID}")))
            .respond_with({
                let calls = Arc::clone(&calls);
                move |_req: &Request| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    ResponseTemplate::new(500)
                }
            })
            .mount(&server)
            .await;

        let client = test_client(&server);
        let result = client.get_bytes(GOLDEN_SMALL_CID).await;

        assert!(matches!(result, Err(IpfsError::NetworkError(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "should try exactly max_attempts times"
        );
    }

    #[tokio::test]
    async fn retries_and_ultimately_rejects_content_that_never_matches_the_cid() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicU32::new(0));
        Mock::given(method("GET"))
            .and(path(format!("/{GOLDEN_SMALL_CID}")))
            .respond_with(AlwaysBody {
                calls: Arc::clone(&calls),
                // Wrong content for this CID on every attempt — simulates a
                // gateway that deterministically serves corrupted bytes.
                body: b"definitely not the real content".to_vec(),
            })
            .mount(&server)
            .await;

        let client = test_client(&server);
        let result = client.get_bytes(GOLDEN_SMALL_CID).await;

        assert!(matches!(
            result,
            Err(IpfsError::VerificationFailed { attempts: 3, .. })
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "should retry a hash mismatch, not trust it"
        );
    }

    #[tokio::test]
    async fn passes_through_unverifiable_content_without_retrying() {
        // A CID shape verify() can't check should be returned as-is on the
        // first successful fetch, not retried. Not valid base32 (doesn't
        // start with 'b') and not valid base58 either (contains characters
        // base58 excludes) — genuinely unparseable, not a real CID shape.
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicU32::new(0));
        let cid = "not-a-real-cid-0O0Il";
        Mock::given(method("GET"))
            .and(path(format!("/{cid}")))
            .respond_with(AlwaysBody {
                calls: Arc::clone(&calls),
                body: b"unverifiable but should pass through".to_vec(),
            })
            .mount(&server)
            .await;

        let client = test_client(&server);
        let bytes = client.get_bytes(cid).await.unwrap();

        assert_eq!(bytes, b"unverifiable but should pass through");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
