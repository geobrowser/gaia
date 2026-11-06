//! The object layout for one `ipfs_cache` row.
//!
//! `u32` big-endian header length, a JSON header (`block`, `space`, `is_errored`, `name`,
//! `has_data`), then the raw payload bytes. Keeping the payload outside the JSON keeps the
//! object byte-comparable with the Postgres `data` column.

use serde::{Deserialize, Serialize};

use crate::{BlobError, BlobResult, object_key};

pub const PREFIX: &str = "ipfs_cache";

/// Object key for an IPFS URI.
pub fn key(uri: &str) -> String {
    object_key(PREFIX, uri)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    block: String,
    space: String,
    is_errored: bool,
    name: Option<String>,
    has_data: bool,
}

/// One `ipfs_cache` row, minus the uri (which is the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub block: String,
    /// Space id; [`encode`] normalizes it to 32 lowercase hex digits, so a uuid rendered
    /// hyphenated (Postgres) and the same id as plain hex (the warmer) encode identically.
    pub space: String,
    pub is_errored: bool,
    pub name: Option<String>,
    pub data: Option<Vec<u8>>,
}

fn normalize_space(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub fn encode(row: &Row) -> Vec<u8> {
    let header = serde_json::to_vec(&Header {
        block: row.block.clone(),
        space: normalize_space(&row.space),
        is_errored: row.is_errored,
        name: row.name.clone(),
        has_data: row.data.is_some(),
    })
    .expect("header serializes");
    let data = row.data.as_deref().unwrap_or(&[]);
    let mut out = Vec::with_capacity(4 + header.len() + data.len());
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    out
}

pub fn decode(bytes: &[u8]) -> BlobResult<Row> {
    let corrupt = |m: &str| BlobError::Corrupt(m.to_string());
    let len_bytes: [u8; 4] = bytes
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| corrupt("short header length"))?;
    let hlen = u32::from_be_bytes(len_bytes) as usize;
    let header_bytes = bytes
        .get(4..4 + hlen)
        .ok_or_else(|| corrupt("truncated header"))?;
    let h: Header =
        serde_json::from_slice(header_bytes).map_err(|e| BlobError::Corrupt(e.to_string()))?;
    let rest = &bytes[4 + hlen..];
    Ok(Row {
        block: h.block,
        space: h.space,
        is_errored: h.is_errored,
        name: h.name,
        data: h.has_data.then(|| rest.to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_and_without_data() {
        let with = Row {
            block: "12".into(),
            space: "00000000000000000000000000000001".into(),
            is_errored: false,
            name: Some("n".into()),
            data: Some(vec![0, 1, 2, 255]),
        };
        assert_eq!(decode(&encode(&with)).unwrap(), with);
        let empty_data = Row {
            data: Some(vec![]),
            ..with.clone()
        };
        assert_eq!(decode(&encode(&empty_data)).unwrap(), empty_data);
        let errored = Row {
            is_errored: true,
            name: None,
            data: None,
            ..with
        };
        assert_eq!(decode(&encode(&errored)).unwrap(), errored);
    }

    #[test]
    fn space_spelling_does_not_change_the_object() {
        let mk = |space: &str| Row {
            block: "1".into(),
            space: space.into(),
            is_errored: false,
            name: None,
            data: Some(vec![1]),
        };
        assert_eq!(
            encode(&mk("0AAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")),
            encode(&mk("0aaaaaaabbbbccccddddeeeeeeeeeeee"))
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"").is_err());
        assert!(decode(&[0, 0, 0, 9, 1]).is_err());
    }

    #[test]
    fn key_uses_prefix() {
        assert_eq!(key("ipfs://Qm1"), "ipfs_cache/ipfs~3A~2F~2FQm1");
    }
}
