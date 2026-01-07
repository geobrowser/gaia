use substreams::Hex;

/// Returns the hex representation of the address in lowercase with 0x prefix
pub fn format_hex(address: &[u8]) -> String {
    format!("0x{}", Hex(address).to_string())
}

/// Extract and validate an IPFS URI from raw event data bytes.
///
/// Searches for "ipfs://" pattern in the data and validates the CID.
/// Returns `Some(uri)` if a valid IPFS URI is found, `None` otherwise.
///
/// Supports:
/// - CIDv0: `Qm` prefix, base58, exactly 46 characters
/// - CIDv1: `b` prefix (e.g., `bafy`), base32, variable length
pub fn extract_ipfs_uri(data: &[u8]) -> Option<String> {
    // Convert to string, replacing invalid UTF-8 with replacement char
    let text = String::from_utf8_lossy(data);

    // Find "ipfs://" pattern
    let start = text.find("ipfs://")?;
    let after_prefix = &text[start + 7..]; // Skip "ipfs://"

    // Extract the CID (alphanumeric characters until whitespace or invalid char)
    let cid: String = after_prefix
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();

    // Validate the CID
    if is_valid_cid(&cid) {
        Some(format!("ipfs://{}", cid))
    } else {
        None
    }
}

/// Validate an IPFS CID.
///
/// - CIDv0: starts with "Qm", base58 (alphanumeric, no 0/O/I/l), exactly 46 chars
/// - CIDv1: starts with "b" (typically "bafy"), base32, variable length (typically 59 chars for base32)
fn is_valid_cid(cid: &str) -> bool {
    if cid.is_empty() {
        return false;
    }

    // CIDv0: Qm + 44 base58 chars = 46 total
    if cid.starts_with("Qm") {
        return cid.len() == 46 && is_base58(cid);
    }

    // CIDv1: starts with 'b' (base32) - typically "bafy" for dag-pb/blake3
    // Length varies but typically 59 chars for base32lower
    if let Some(rest) = cid.strip_prefix('b') {
        // Base32 uses a-z and 2-7
        return cid.len() >= 50 && is_base32_lower(rest);
    }

    false
}

/// Check if string is valid base58 (no 0, O, I, l)
fn is_base58(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l')
}

/// Check if string is valid base32 lowercase (a-z, 2-7)
fn is_base32_lower(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cidv0() {
        let data = b"ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
        let result = extract_ipfs_uri(data);
        assert_eq!(
            result,
            Some("ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_string())
        );
    }

    #[test]
    fn test_extract_cidv1() {
        let data = b"ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3okuez3djvxfzq";
        let result = extract_ipfs_uri(data);
        assert_eq!(
            result,
            Some("ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3okuez3djvxfzq".to_string())
        );
    }

    #[test]
    fn test_extract_with_abi_padding() {
        // Simulates ABI-encoded data with padding before/after the URI
        let mut data = vec![0u8; 64]; // 64 bytes of zeros (ABI offset + length)
        data.extend_from_slice(b"ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG");
        data.extend_from_slice(&[0u8; 32]); // Trailing padding

        let result = extract_ipfs_uri(&data);
        assert_eq!(
            result,
            Some("ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_string())
        );
    }

    #[test]
    fn test_extract_no_ipfs_prefix() {
        let data = b"QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
        let result = extract_ipfs_uri(data);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_invalid_cid_too_short() {
        let data = b"ipfs://QmTooShort";
        let result = extract_ipfs_uri(data);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_inline_content() {
        // Real example: inline edit content instead of IPFS URI
        let data = b"Down the Rabbit-Hole";
        let result = extract_ipfs_uri(data);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_cidv0_wrong_length() {
        // CIDv0 must be exactly 46 chars
        let data = b"ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdGextra";
        let result = extract_ipfs_uri(data);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_cidv1_too_short() {
        // CIDv1 must be at least 50 chars
        let data = b"ipfs://bafyshort";
        let result = extract_ipfs_uri(data);
        assert_eq!(result, None);
    }

    #[test]
    fn test_is_valid_cid_v0() {
        assert!(is_valid_cid(
            "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"
        ));
    }

    #[test]
    fn test_is_valid_cid_v1() {
        assert!(is_valid_cid(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3okuez3djvxfzq"
        ));
    }

    #[test]
    fn test_is_valid_cid_empty() {
        assert!(!is_valid_cid(""));
    }

    #[test]
    fn test_is_base58() {
        assert!(is_base58("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"));
        assert!(!is_base58("Qm0Invalid")); // Contains 0
        assert!(!is_base58("QmOInvalid")); // Contains O
        assert!(!is_base58("QmIInvalid")); // Contains I
        assert!(!is_base58("QmlInvalid")); // Contains l
    }
}
