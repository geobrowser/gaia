//! Redact credentials from connection URLs before they are logged.
//!
//! Connection strings such as `OPENSEARCH_URL` and `DATABASE_URL` embed a
//! username and password as URL userinfo (`scheme://user:password@host:port`).
//! Logging the raw value leaks the credential into log aggregation, CI output
//! and anything else that collects stdout/stderr. `redact_url_credentials`
//! keeps only what's useful for diagnostics — scheme, host and port — and
//! drops the userinfo entirely.
//!
//! Input that can't be parsed as a URL falls back to a fixed placeholder, so
//! a malformed value never leaks verbatim either.

/// Placeholder returned for a URL that could not be parsed.
pub const REDACTED_INVALID_URL: &str = "[redacted-invalid-url]";

/// Strip userinfo (username/password) from `raw_url`, returning only the
/// scheme, host and port.
///
/// ```
/// use search_indexer_shared::redact_url_credentials;
///
/// assert_eq!(
///     redact_url_credentials("https://doadmin:hunter2@search.example.com:25060"),
///     "https://search.example.com:25060"
/// );
/// assert_eq!(
///     redact_url_credentials("http://localhost:9200"),
///     "http://localhost:9200"
/// );
/// assert_eq!(redact_url_credentials("not a url"), "[redacted-invalid-url]");
/// ```
pub fn redact_url_credentials(raw_url: &str) -> String {
    match url::Url::parse(raw_url) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            let host = match parsed.host_str() {
                Some(h) => h,
                None => return REDACTED_INVALID_URL.to_string(),
            };
            match parsed.port() {
                Some(port) => format!("{}://{}:{}", scheme, host, port),
                None => format!("{}://{}", scheme, host),
            }
        }
        Err(_) => REDACTED_INVALID_URL.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_username_and_password() {
        assert_eq!(
            redact_url_credentials("https://doadmin:hunter2@search.example.com:25060"),
            "https://search.example.com:25060"
        );
    }

    #[test]
    fn leaves_url_without_userinfo_unchanged() {
        assert_eq!(
            redact_url_credentials("http://localhost:9200"),
            "http://localhost:9200"
        );
    }

    #[test]
    fn drops_path_query_and_fragment_along_with_userinfo() {
        // A non-default port, so `Url::port()` reports it explicitly rather
        // than treating it as implied by the scheme.
        assert_eq!(
            redact_url_credentials("https://user:pass@host:8443/some/path?query=1#frag"),
            "https://host:8443"
        );
    }

    #[test]
    fn handles_special_characters_in_password() {
        // '@', ':' and '/' in a password must be percent-encoded in a valid
        // URL, but the redaction must never leak the decoded or encoded
        // secret either way.
        let url = "postgres://admin:p%40ss%2Fw%3Ard@db.example.com:5432/mydb";
        let result = redact_url_credentials(url);
        assert_eq!(result, "postgres://db.example.com:5432");
        assert!(!result.contains("admin"));
        assert!(!result.contains("p@ss"));
        assert!(!result.contains("p%40ss"));
    }

    #[test]
    fn omits_port_when_not_specified() {
        assert_eq!(
            redact_url_credentials("https://user:pass@host.example.com/path"),
            "https://host.example.com"
        );
    }

    #[test]
    fn falls_back_to_placeholder_for_unparseable_input() {
        assert_eq!(
            redact_url_credentials("not a url at all"),
            REDACTED_INVALID_URL
        );
        assert_eq!(redact_url_credentials(""), REDACTED_INVALID_URL);
        assert_eq!(
            redact_url_credentials("localhost:9200"),
            REDACTED_INVALID_URL
        );
    }
}
