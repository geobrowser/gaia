//! Retry utilities for OpenSearch operations.
//!
//! Provides exponential backoff with jitter for transient failures
//! (transport errors, 429, 502, 503, 504).

use std::time::{Duration, SystemTime};

use tracing::warn;

/// Configuration for retry behavior with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 means no retries).
    pub max_retries: u32,
    /// Base delay in milliseconds (doubled each attempt).
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (caps the exponential growth).
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 5000,
        }
    }
}

/// Returns true if the HTTP status code is retryable (transient server/rate-limit error).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Compute the backoff delay for a given attempt (1-indexed).
///
/// Uses exponential backoff: `min(base * 2^(attempt-1), max_delay)` with ±25% jitter
/// derived from `SystemTime` nanoseconds to avoid thundering herd.
pub fn compute_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let exp = config
        .base_delay_ms
        .saturating_mul(1u64 << (attempt - 1).min(31));
    let capped = exp.min(config.max_delay_ms);

    // Deterministic jitter: ±25% based on system time nanoseconds
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Map nanos to range [0.75, 1.25]
    let jitter_factor = 0.75 + (nanos as f64 / u32::MAX as f64) * 0.5;
    let jittered = (capped as f64 * jitter_factor) as u64;

    Duration::from_millis(jittered.min(config.max_delay_ms))
}

/// Sleep for the computed backoff delay, logging the retry attempt.
pub async fn backoff_sleep(attempt: u32, config: &RetryConfig, context: &str) {
    let delay = compute_delay(attempt, config);
    warn!(
        context = context,
        attempt = attempt,
        max_retries = config.max_retries,
        delay_ms = delay.as_millis() as u64,
        "Retrying after transient error"
    );
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_status() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(409));
        assert!(!is_retryable_status(500));
    }

    #[test]
    fn test_compute_delay_exponential_growth() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_ms: 100,
            max_delay_ms: 10000,
        };

        // Attempt 1: base * 2^0 = 100ms ±25% → [75, 125]
        let d1 = compute_delay(1, &config);
        assert!(d1.as_millis() >= 75 && d1.as_millis() <= 125);

        // Attempt 2: base * 2^1 = 200ms ±25% → [150, 250]
        let d2 = compute_delay(2, &config);
        assert!(d2.as_millis() >= 150 && d2.as_millis() <= 250);

        // Attempt 3: base * 2^2 = 400ms ±25% → [300, 500]
        let d3 = compute_delay(3, &config);
        assert!(d3.as_millis() >= 300 && d3.as_millis() <= 500);
    }

    #[test]
    fn test_compute_delay_respects_max() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay_ms: 100,
            max_delay_ms: 500,
        };

        // Attempt 5: base * 2^4 = 1600ms, capped at 500ms ±25% → [375, 500]
        let d = compute_delay(5, &config);
        assert!(d.as_millis() <= 500);
    }

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 5000);
    }
}
