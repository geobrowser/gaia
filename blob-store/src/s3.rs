use std::time::Duration;

use crate::sigv4::{self, sha256_hex};
use crate::{BlobError, BlobResult, BlobStore};

/// Connection settings. The credentials come from the environment (in the cluster, from a
/// Secret via `secretKeyRef`); `Debug` deliberately omits them.
#[derive(Clone)]
pub struct S3Config {
    /// e.g. `https://nyc3.digitaloceanspaces.com` (scheme required, no bucket).
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl S3Config {
    /// `BLOB_S3_ENDPOINT`, `BLOB_S3_BUCKET`, `BLOB_S3_ACCESS_KEY`, `BLOB_S3_SECRET_KEY`,
    /// optional `BLOB_S3_REGION` (default `us-east-1`, which Spaces accepts for signing).
    pub fn from_env() -> BlobResult<Self> {
        let req = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| BlobError::Config(format!("{k} is not set")))
        };
        Ok(Self {
            endpoint: req("BLOB_S3_ENDPOINT")?,
            bucket: req("BLOB_S3_BUCKET")?,
            access_key: req("BLOB_S3_ACCESS_KEY")?,
            secret_key: req("BLOB_S3_SECRET_KEY")?,
            region: std::env::var("BLOB_S3_REGION")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "us-east-1".to_string()),
        })
    }
}

/// S3-compatible backend (path-style, SigV4). A PUT replaces the object atomically.
pub struct S3BlobStore {
    cfg: S3Config,
    http: reqwest::Client,
    host: String,
    base: String,
}

impl S3BlobStore {
    pub fn new(mut cfg: S3Config) -> BlobResult<Self> {
        if !cfg.endpoint.contains("://") {
            cfg.endpoint = format!("https://{}", cfg.endpoint);
        }
        let base = cfg.endpoint.trim_end_matches('/').to_string();
        let host = base
            .split_once("://")
            .map(|(_, h)| h)
            .unwrap_or(&base)
            .to_string();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| BlobError::Config(e.to_string()))?;
        Ok(Self {
            cfg,
            http,
            host,
            base,
        })
    }

    async fn send(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<&[u8]>,
    ) -> BlobResult<reqwest::Response> {
        let canonical_uri = format!("/{}/{}", self.cfg.bucket, key);
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let payload_hash = sha256_hex(body.unwrap_or(b""));
        let signed = sigv4::sign(
            method.as_str(),
            &canonical_uri,
            &[
                ("host", &self.host),
                ("x-amz-content-sha256", &payload_hash),
                ("x-amz-date", &amz_date),
            ],
            &payload_hash,
            &self.cfg.secret_key,
            &self.cfg.region,
            &amz_date,
        );
        let auth = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/s3/aws4_request, SignedHeaders={}, Signature={}",
            self.cfg.access_key,
            &amz_date[..8],
            self.cfg.region,
            signed.signed_headers,
            signed.signature
        );
        let mut req = self
            .http
            .request(method, format!("{}{}", self.base, canonical_uri))
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("authorization", auth);
        if let Some(b) = body {
            req = req.body(b.to_vec());
        }
        req.send().await.map_err(|e| BlobError::Http(e.to_string()))
    }
}

fn status_err(op: &str, key: &str, status: reqwest::StatusCode) -> BlobError {
    BlobError::Http(format!("{op} {key}: status {status}"))
}

#[async_trait::async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> BlobResult<()> {
        let r = self.send(reqwest::Method::PUT, key, Some(bytes)).await?;
        if r.status().is_success() {
            Ok(())
        } else {
            Err(status_err("put", key, r.status()))
        }
    }

    async fn get(&self, key: &str) -> BlobResult<Option<Vec<u8>>> {
        let r = self.send(reqwest::Method::GET, key, None).await?;
        match r.status() {
            s if s.is_success() => Ok(Some(
                r.bytes()
                    .await
                    .map_err(|e| BlobError::Http(e.to_string()))?
                    .to_vec(),
            )),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            s => Err(status_err("get", key, s)),
        }
    }

    async fn exists(&self, key: &str) -> BlobResult<bool> {
        let r = self.send(reqwest::Method::HEAD, key, None).await?;
        match r.status() {
            s if s.is_success() => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            s => Err(status_err("head", key, s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Tiny in-process HTTP/1.1 object server: enough to round-trip PUT/GET/HEAD and to
    /// check that every request carries a SigV4 Authorization header.
    async fn serve() -> (String, Arc<Mutex<HashMap<String, Vec<u8>>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let store: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let st = store.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let st = st.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let (head, mut body) = loop {
                        let n = sock.read(&mut tmp).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break (
                                String::from_utf8_lossy(&buf[..i]).to_string(),
                                buf[i + 4..].to_vec(),
                            );
                        }
                    };
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap().split(' ');
                    let (method, path) = (first.next().unwrap(), first.next().unwrap().to_string());
                    let mut len = 0usize;
                    let mut authed = false;
                    for l in lines {
                        let l = l.to_ascii_lowercase();
                        if let Some(v) = l.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap();
                        }
                        if l.starts_with("authorization: aws4-hmac-sha256 credential=ak/")
                            && l.contains("signedheaders=host;x-amz-content-sha256;x-amz-date")
                        {
                            authed = true;
                        }
                    }
                    while body.len() < len {
                        let n = sock.read(&mut tmp).await.unwrap();
                        body.extend_from_slice(&tmp[..n]);
                    }
                    let resp: Vec<u8> = if !authed {
                        b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n".to_vec()
                    } else {
                        match method {
                            "PUT" => {
                                st.lock().unwrap().insert(path, body);
                                b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n".to_vec()
                            }
                            "GET" | "HEAD" => match st.lock().unwrap().get(&path) {
                                Some(b) => {
                                    let mut r = format!(
                                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                                        b.len()
                                    )
                                    .into_bytes();
                                    if method == "GET" {
                                        r.extend_from_slice(b);
                                    }
                                    r
                                }
                                None => {
                                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec()
                                }
                            },
                            _ => b"HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\n\r\n"
                                .to_vec(),
                        }
                    };
                    sock.write_all(&resp).await.unwrap();
                });
            }
        });
        (format!("http://{addr}"), store)
    }

    #[tokio::test]
    async fn round_trip_against_local_server() {
        let (endpoint, objects) = serve().await;
        let s = S3BlobStore::new(S3Config {
            endpoint,
            bucket: "b".into(),
            access_key: "ak".into(),
            secret_key: "sk".into(),
            region: "us-east-1".into(),
        })
        .unwrap();
        let key = crate::object_key("ipfs_cache", "ipfs://Qm1");
        assert!(!s.exists(&key).await.unwrap());
        assert_eq!(s.get(&key).await.unwrap(), None);
        s.put(&key, b"hello").await.unwrap();
        s.put(&key, b"hello world").await.unwrap();
        assert!(s.exists(&key).await.unwrap());
        assert_eq!(s.get(&key).await.unwrap(), Some(b"hello world".to_vec()));
        assert!(objects.lock().unwrap().contains_key(&format!("/b/{key}")));
    }

    #[test]
    fn config_debug_hides_credentials() {
        let c = S3Config {
            endpoint: "https://e".into(),
            bucket: "b".into(),
            access_key: "AKSECRET".into(),
            secret_key: "SKSECRET".into(),
            region: "r".into(),
        };
        let d = format!("{c:?}");
        assert!(!d.contains("AKSECRET") && !d.contains("SKSECRET"));
    }
}
