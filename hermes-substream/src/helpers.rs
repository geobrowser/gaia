use ethabi::{ParamType, Token};
use substreams::Hex;

/// Returns the hex representation of the address in lowercase with 0x prefix
pub fn format_hex(address: &[u8]) -> String {
    format!("0x{}", Hex(address).to_string())
}

/// Function selector for publish(bytes32, bytes, bytes)
const PUBLISH_SELECTOR: [u8; 4] = [0x6b, 0x47, 0xf6, 0x1a];

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

/// Extract IPFS URIs from PROPOSAL_CREATED action data.
///
/// The data is ABI-encoded as: `(bytes16 proposalId, uint8 votingMode, Action[])`
/// where each Action is `(address toAddress, bytes16 toSpaceId, uint256 value, bytes data)`.
///
/// For Publish actions (selector 0x6b47f61a), the data contains:
/// `publish(bytes32 topic, bytes contentUri, bytes metadata)`
///
/// Returns a list of valid IPFS URIs found in Publish actions.
pub fn extract_proposal_publish_uris(data: &[u8]) -> Vec<String> {
    let mut uris = Vec::new();

    // Try to decode the proposal data
    let actions = match decode_proposal_actions(data) {
        Some(actions) => actions,
        None => return uris,
    };

    // For each action, check if it's a Publish action
    for action_data in actions {
        if action_data.len() < 4 {
            continue;
        }

        // Check if this is a Publish action
        let selector: [u8; 4] = action_data[0..4].try_into().unwrap_or([0; 4]);
        if selector != PUBLISH_SELECTOR {
            continue;
        }

        // Decode publish args: (bytes32 topic, bytes contentUri, bytes metadata)
        if let Some(uri) = decode_publish_content_uri(&action_data[4..])
            && let Some(valid_uri) = extract_ipfs_uri(uri.as_bytes())
        {
            uris.push(valid_uri);
        }
    }

    uris
}

/// Decode proposal actions from PROPOSAL_CREATED data.
/// Returns the action calldata bytes for each action.
fn decode_proposal_actions(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    if data.is_empty() {
        return None;
    }

    // Try decoding with possible bytes wrapping
    if let Some(actions) = decode_proposal_actions_inner(data) {
        return Some(actions);
    }

    // Try unwrapping once and decoding
    if let Some(unwrapped) = unwrap_bytes_once(data)
        && let Some(actions) = decode_proposal_actions_inner(&unwrapped)
    {
        return Some(actions);
    }

    None
}

/// Inner decode function for proposal actions.
fn decode_proposal_actions_inner(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    // ABI schema: (bytes16 proposalId, uint8 votingMode, Action[])
    // where Action is (address toAddress, bytes16 toSpaceId, uint256 value, bytes data)
    let params = [
        ParamType::FixedBytes(16),
        ParamType::Uint(8),
        ParamType::Array(Box::new(ParamType::Tuple(vec![
            ParamType::Address,        // toAddress (call target; resolved onchain)
            ParamType::FixedBytes(16), // toSpaceId (call target; resolved onchain)
            ParamType::Uint(256),
            ParamType::Bytes,
        ]))),
    ];

    let tokens = ethabi::decode(&params, data).ok()?;

    // Extract action calldata (fields[3]) from the actions array
    let actions = match &tokens.get(2)? {
        Token::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Token::Tuple(fields) if fields.len() == 4 => match &fields[3] {
                    Token::Bytes(bytes) => Some(bytes.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect(),
        _ => return None,
    };

    Some(actions)
}

/// Decode the content_uri from publish action calldata (after selector).
///
/// Publish args: (bytes32 topic, bytes contentUri, bytes metadata)
fn decode_publish_content_uri(data: &[u8]) -> Option<String> {
    // Decode (bytes32, bytes, bytes)
    let params = [
        ParamType::FixedBytes(32),
        ParamType::Bytes,
        ParamType::Bytes,
    ];

    let tokens = ethabi::decode(&params, data).ok()?;

    // Get the contentUri (second element)
    let content_uri_bytes = match &tokens.get(1)? {
        Token::Bytes(bytes) => bytes.clone(),
        _ => return None,
    };

    // Try to unwrap if wrapped in another bytes encoding
    let content_uri_bytes = unwrap_bytes_if_needed(&content_uri_bytes);

    // Convert to UTF-8 string, stripping null bytes
    let cleaned: Vec<u8> = content_uri_bytes.into_iter().filter(|b| *b != 0).collect();
    String::from_utf8(cleaned).ok()
}

/// Unwrap bytes if they appear to be ABI-encoded bytes wrapper.
fn unwrap_bytes_if_needed(data: &[u8]) -> Vec<u8> {
    if let Some(unwrapped) = unwrap_bytes_once(data) {
        // Try one more level
        if let Some(double_unwrapped) = unwrap_bytes_once(&unwrapped) {
            return double_unwrapped;
        }
        return unwrapped;
    }
    data.to_vec()
}

/// Try to unwrap ABI-encoded bytes (offset + length + data).
fn unwrap_bytes_once(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 64 {
        return None;
    }

    // Check for ABI bytes encoding: first 24 bytes should be zero (offset padding)
    // bytes 24-32: offset (should be 32)
    // bytes 32-56: should be zero (length padding)
    // bytes 56-64: length
    if data[0..24].iter().any(|b| *b != 0) || data[32..56].iter().any(|b| *b != 0) {
        return None;
    }

    let offset = u64::from_be_bytes(data[24..32].try_into().ok()?) as usize;
    if offset != 32 {
        return None;
    }

    let len = u64::from_be_bytes(data[56..64].try_into().ok()?) as usize;
    let start = 64;
    let end = start + len;
    if end > data.len() {
        return None;
    }

    Some(data[start..end].to_vec())
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
