//! HMAC signing and HTTP delivery logic.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::DeliveryError;

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256 hex digest of a payload using the given secret.
///
/// Returns an error if the secret is somehow rejected by the HMAC implementation
/// (in practice HMAC-SHA256 accepts any key length, so this is defensive).
pub fn compute_hmac(secret: &str, payload: &[u8]) -> Result<String, DeliveryError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| DeliveryError::Hmac(format!("invalid HMAC key: {}", e)))?;
    mac.update(payload);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Deliver a notification payload to a webhook URL with HMAC signature.
///
/// The idempotency key is included in the JSON body (not as a header).
/// Returns `Ok(status_code)` on any HTTP response, or `Err` on network failure.
pub async fn deliver_webhook(
    client: &reqwest::Client,
    url: &str,
    secret: &str,
    payload: &[u8],
) -> Result<u16, DeliveryError> {
    let signature = compute_hmac(secret, payload)?;

    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-Geo-Signature", format!("sha256={}", signature))
        .body(payload.to_vec())
        .send()
        .await?;

    Ok(response.status().as_u16())
}

/// Check if a response status code indicates the delivery should be considered successful.
pub fn is_success(status: u16) -> bool {
    (200..300).contains(&status) || status == 409
}

/// Check if a response status code indicates the delivery should be retried.
pub fn should_retry(status: u16) -> bool {
    status >= 500 || status == 429
}

/// Calculate exponential backoff delay in seconds for a given attempt number.
///
/// Formula: min(30 * 2^(attempt-1), 172800) seconds.
/// Attempt 1 → 30s, attempt 2 → 60s, attempt 3 → 120s, ..., capped at 48 hours.
pub fn backoff_seconds(attempt: i32) -> i64 {
    let delay = 30i64.saturating_mul(1i64.checked_shl((attempt - 1) as u32).unwrap_or(i64::MAX));
    delay.min(172_800)
}

/// Maximum number of delivery attempts before marking as failed.
pub const MAX_RETRIES: i32 = 100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_signature() {
        let secret = "test-secret";
        let payload = b"hello world";
        let sig = compute_hmac(secret, payload).expect("hmac should succeed");

        // HMAC-SHA256 of "hello world" with key "test-secret" is deterministic
        assert_eq!(sig.len(), 64); // hex-encoded SHA256 is 64 chars
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify same input produces same output
        let sig2 = compute_hmac(secret, payload).expect("hmac should succeed");
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_hmac_signature_different_secrets() {
        let payload = b"same payload";
        let sig1 = compute_hmac("secret-a", payload).expect("hmac should succeed");
        let sig2 = compute_hmac("secret-b", payload).expect("hmac should succeed");

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_should_retry_on_5xx() {
        assert!(should_retry(500));
        assert!(should_retry(502));
        assert!(should_retry(503));
        assert!(should_retry(429));
    }

    #[test]
    fn test_should_not_retry_on_4xx() {
        assert!(!should_retry(400));
        assert!(!should_retry(401));
        assert!(!should_retry(404));
    }

    #[test]
    fn test_should_not_retry_on_409() {
        // 409 Conflict is treated as "already delivered" (duplicate), not a retry
        assert!(!should_retry(409));
    }

    #[test]
    fn test_is_success() {
        assert!(is_success(200));
        assert!(is_success(201));
        assert!(is_success(204));
        assert!(is_success(409)); // duplicate = success
        assert!(!is_success(400));
        assert!(!is_success(500));
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        assert_eq!(backoff_seconds(1), 30); // 30 * 2^0
        assert_eq!(backoff_seconds(2), 60); // 30 * 2^1
        assert_eq!(backoff_seconds(3), 120); // 30 * 2^2
        assert_eq!(backoff_seconds(4), 240); // 30 * 2^3
        assert_eq!(backoff_seconds(5), 480); // 30 * 2^4
        assert_eq!(backoff_seconds(6), 960); // 30 * 2^5
        assert_eq!(backoff_seconds(7), 1920); // 30 * 2^6
        assert_eq!(backoff_seconds(8), 3840); // 30 * 2^7
        assert_eq!(backoff_seconds(13), 122880); // 30 * 2^12
        assert_eq!(backoff_seconds(14), 172800); // capped at 48hr
        assert_eq!(backoff_seconds(15), 172800); // still capped
        assert_eq!(backoff_seconds(100), 172800); // still capped
    }
}
