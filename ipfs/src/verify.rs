//! CID verification for content fetched from an IPFS gateway.
//!
//! Real GRC-20 edit CIDs observed in production come in two shapes:
//!
//! - **CIDv0** (`Qm...`, base58btc): always sha2-256 over a UnixFS `File`
//!   node wrapped in a dag-pb block — never a hash of the raw bytes
//!   directly. Produced by `ipfs add` with default settings (e.g. Filebase's
//!   Kubo-compatible endpoint — see `api/src/services/ipfs.ts`).
//! - **CIDv1** (`bafkrei...`, base32 multibase 'b'): observed in production
//!   almost exclusively with the `raw` codec, where the multihash digest
//!   *is* directly `sha2-256(raw_bytes)` — no wrapping at all. CIDv1 with
//!   the `dag-pb` codec is also supported here for completeness, using the
//!   same UnixFS reconstruction as CIDv0, in case an older or differently
//!   configured client ever produces one.
//!
//! Both codec/version combinations were validated against real production
//! (CID, bytes) pairs during development — see this module's tests.
//!
//! Every GRC-20 edit observed in production is well under the 256 KiB
//! default UnixFS chunk size, so single-chunk (no `Links`) dag-pb
//! reconstruction covers the entire real dag-pb workload. `raw` codec CIDs
//! have no such limit — a raw-codec CID is by construction never chunked
//! (a chunked/multi-block file's root CID is always a linking dag-pb node).
//! Anything this module can't confidently parse is reported as
//! [`Verification::Unsupported`] rather than guessed at.

use sha2::{Digest, Sha256};

/// Kubo's default max chunk size — content at or under this fits in a single
/// UnixFS leaf with no `Links`, the only shape this module can reconstruct
/// for the dag-pb codec.
const MAX_SINGLE_CHUNK_BYTES: usize = 256 * 1024;

/// sha2-256 multihash function code (see the multiformats multihash table).
const MULTIHASH_SHA2_256: u64 = 0x12;
/// Digest length in bytes for sha2-256.
const MULTIHASH_SHA2_256_LEN: u64 = 0x20;

/// Multicodec code for raw binary (content-addressed directly, no wrapping).
const CODEC_RAW: u64 = 0x55;
/// Multicodec code for a dag-pb (MerkleDAG protobuf / UnixFS) node.
const CODEC_DAG_PB: u64 = 0x70;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// The bytes hash to exactly the content this CID identifies.
    Verified,
    /// The bytes were successfully reconstructed into the block shape this
    /// CID's codec implies, but its digest does not match. The fetch
    /// returned the wrong (or corrupted) content.
    Mismatch,
    /// This CID isn't one of the shapes this module understands (see module
    /// docs), or the content is larger than a single dag-pb chunk —
    /// verification isn't implemented for that case, so no claim is made
    /// either way.
    Unsupported,
}

/// A parsed, not-yet-verified CID: which block shape to expect, and the
/// multihash digest it should hash to.
struct ParsedCid {
    codec: u64,
    digest: Vec<u8>,
}

/// Verify that `bytes` are the actual content identified by `cid`.
///
/// `cid` may be given with or without an `ipfs://` prefix.
pub fn verify(cid: &str, bytes: &[u8]) -> Verification {
    let cid = cid.strip_prefix("ipfs://").unwrap_or(cid);

    let Some(parsed) = parse_cid(cid) else {
        return Verification::Unsupported;
    };

    let actual_digest = match parsed.codec {
        CODEC_RAW => Sha256::digest(bytes).to_vec(),
        CODEC_DAG_PB => {
            if bytes.len() > MAX_SINGLE_CHUNK_BYTES {
                return Verification::Unsupported;
            }
            Sha256::digest(unixfs_file_block(bytes)).to_vec()
        }
        _ => return Verification::Unsupported,
    };

    if actual_digest == parsed.digest {
        Verification::Verified
    } else {
        Verification::Mismatch
    }
}

/// Parse a CIDv0 or CIDv1 string into its codec and expected sha2-256
/// digest. Returns `None` for anything not in one of those two shapes, or
/// using a hash function other than sha2-256.
fn parse_cid(cid: &str) -> Option<ParsedCid> {
    // Multibase 'b' (lowercase) and 'B' (uppercase) both denote base32
    // (RFC4648, no padding) — 'b' is by far the most common CIDv1 string
    // encoding in practice, but 'B' is equally valid per the multibase spec.
    // `base32_decode_nopad` itself is already case-insensitive, so only the
    // prefix check needs to accept both.
    if cid.starts_with('b') || cid.starts_with('B') {
        let rest = &cid[1..];
        let bytes = base32_decode_nopad(rest)?;
        let mut pos = 0;
        let (version, n) = read_varint(&bytes[pos..])?;
        pos += n;
        if version != 1 {
            return None;
        }
        let (codec, n) = read_varint(&bytes[pos..])?;
        pos += n;
        let (mh_code, n) = read_varint(&bytes[pos..])?;
        pos += n;
        let (mh_len, n) = read_varint(&bytes[pos..])?;
        pos += n;
        if mh_code != MULTIHASH_SHA2_256 || mh_len != MULTIHASH_SHA2_256_LEN {
            return None;
        }
        let digest_end = pos.checked_add(mh_len as usize)?;
        // Reject trailing bytes past the digest — a canonical CIDv1 multihash
        // ends exactly here; anything left over is a malformed or
        // non-canonical encoding we shouldn't guess about.
        if digest_end != bytes.len() {
            return None;
        }
        let digest = bytes[pos..digest_end].to_vec();
        return Some(ParsedCid { codec, digest });
    }

    // CIDv0: base58btc encoding of exactly a 34-byte sha2-256 multihash
    // (<code=0x12><len=0x20><32-byte digest>), always implicitly dag-pb.
    let decoded = bs58::decode(cid).into_vec().ok()?;
    if decoded.len() != 34
        || decoded[0] != MULTIHASH_SHA2_256 as u8
        || decoded[1] != MULTIHASH_SHA2_256_LEN as u8
    {
        return None;
    }
    Some(ParsedCid {
        codec: CODEC_DAG_PB,
        digest: decoded[2..].to_vec(),
    })
}

/// Decode RFC4648 base32 (lowercase alphabet, no padding) — the encoding
/// multibase prefix `b` denotes.
fn base32_decode_nopad(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

    let mut bits: u64 = 0;
    let mut bit_count: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 5 / 8);

    for ch in s.bytes() {
        let value = ALPHABET
            .iter()
            .position(|&c| c == ch.to_ascii_lowercase())? as u64;
        bits = (bits << 5) | value;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xff) as u8);
        }
    }

    Some(out)
}

/// Read an unsigned LEB128 varint, returning `(value, bytes_consumed)`.
fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    // A u64 needs at most 10 groups of 7 bits (70 > 64). Bounding the loop
    // (rather than trusting the continuation bit alone) keeps a malformed
    // CID with an unbroken run of continuation bytes from shifting past the
    // width of `value` — `checked_shl` would otherwise return `None` there
    // and this function would just report a bogus-but-harmless failure, but
    // bounding it up front makes that guarantee explicit rather than
    // incidental.
    let mut value: u64 = 0;
    for (i, &byte) in bytes.iter().take(10).enumerate() {
        let shifted = ((byte & 0x7f) as u64).checked_shl(7 * i as u32)?;
        value |= shifted;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Reconstruct the dag-pb block Kubo produces for a single-chunk UnixFS file
/// (no `Links`), given the raw file content.
///
/// Wire shapes (proto2, from go-ipfs's `unixfs.pb.go` / `merkledag.pb.go`):
/// ```proto
/// message Data { required DataType Type = 1; optional bytes Data = 2; optional uint64 filesize = 3; ... }
/// message PBNode { repeated PBLink Links = 2; optional bytes Data = 1; }
/// ```
/// `DataType::File = 2`. Validated against real production CIDs during
/// development — see `ipfs/src/verify.rs` tests.
fn unixfs_file_block(content: &[u8]) -> Vec<u8> {
    const TYPE_FILE: u64 = 2;

    let mut unixfs_data = Vec::with_capacity(content.len() + 16);
    write_varint_field(&mut unixfs_data, 1, TYPE_FILE);
    write_bytes_field(&mut unixfs_data, 2, content);
    write_varint_field(&mut unixfs_data, 3, content.len() as u64);

    let mut node = Vec::with_capacity(unixfs_data.len() + 8);
    write_bytes_field(&mut node, 1, &unixfs_data);
    node
}

/// Encode a protobuf varint (LEB128, unsigned) field: `(field_num << 3) | 0`.
fn write_varint_field(out: &mut Vec<u8>, field_num: u32, value: u64) {
    write_varint(out, (field_num as u64) << 3);
    write_varint(out, value);
}

/// Encode a protobuf length-delimited field: `(field_num << 3) | 2`.
fn write_bytes_field(out: &mut Vec<u8>, field_num: u32, value: &[u8]) {
    write_varint(out, ((field_num as u64) << 3) | 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            out.push(byte | 0x80);
        } else {
            out.push(byte);
            break;
        }
    }
}

/// Real production (CID, bytes) fixtures, reused by this module's tests and
/// by `lib.rs`'s retry tests.
#[cfg(test)]
pub(crate) mod fixtures {
    // Real production payload + CID (space 7570a0ba7552e6806e0751c2ad105754,
    // proposal 4d514f70a1114cf78c2c5a4e20c09f39 — the "Encoding error"
    // incident this module was built to prevent). 123 bytes; single-byte
    // varint for the UnixFS `Data` length field.
    pub(crate) const GOLDEN_SMALL_CID: &str = "QmYdCDY6MQe9ve1HbUiwo3fLLB9eXdCUiWb9bYL8PJ2LXS";
    pub(crate) const GOLDEN_SMALL_BYTES: &[u8] = &[
        0x47, 0x52, 0x43, 0x32, 0x00, 0x76, 0x2f, 0xb5, 0x48, 0x7b, 0x43, 0x42, 0x50, 0x95, 0x28,
        0x60, 0x55, 0x3b, 0x58, 0xbb, 0x32, 0x07, 0x50, 0x55, 0x62, 0x6c, 0x69, 0x73, 0x68, 0x01,
        0xf3, 0xda, 0xb7, 0x9c, 0xb5, 0xa3, 0xd9, 0xd1, 0x75, 0x96, 0x56, 0xdd, 0x53, 0x61, 0xd1,
        0xc6, 0x80, 0xbb, 0xd8, 0xab, 0x94, 0xc3, 0xab, 0x06, 0x01, 0xa1, 0x26, 0xca, 0x53, 0x0c,
        0x8e, 0x48, 0xd5, 0xb8, 0x88, 0x82, 0xc7, 0x34, 0xc3, 0x89, 0x35, 0x05, 0x00, 0x01, 0x09,
        0x0a, 0xda, 0xc0, 0xfc, 0xa4, 0x82, 0x2e, 0x8e, 0x71, 0x92, 0x63, 0xe6, 0x76, 0x20, 0xec,
        0x00, 0x01, 0xe5, 0x23, 0x28, 0x89, 0xff, 0x85, 0x49, 0x45, 0xb5, 0xb7, 0xbb, 0x5d, 0x20,
        0x77, 0x92, 0x11, 0x00, 0x00, 0x01, 0x02, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0xff, 0xff,
        0xff, 0xff, 0x0f,
    ];

    // Second real production payload + CID (space 7570a0ba7552e6806e0751c2ad105754,
    // "Create debates page type", from the 07-17 Preston incident). 248 bytes;
    // exercises the 2-byte varint length-field path the 123-byte fixture above
    // doesn't reach.
    pub(crate) const GOLDEN_LARGE_CID: &str = "QmXEpL18P3gkBdgcm4obxQQfVAEWTYCJV8pmT4KVfmJ5Xo";
    pub(crate) const GOLDEN_LARGE_BYTES: &[u8] = &[
        0x47, 0x52, 0x43, 0x32, 0x00, 0x9a, 0x43, 0x71, 0xd5, 0x9e, 0x1c, 0x44, 0xd2, 0xba, 0xd1,
        0x9e, 0x66, 0x8d, 0xec, 0x09, 0x60, 0x18, 0x43, 0x72, 0x65, 0x61, 0x74, 0x65, 0x20, 0x64,
        0x65, 0x62, 0x61, 0x74, 0x65, 0x73, 0x20, 0x70, 0x61, 0x67, 0x65, 0x20, 0x74, 0x79, 0x70,
        0x65, 0x01, 0xf3, 0xda, 0xb7, 0x9c, 0xb5, 0xa3, 0xd9, 0xd1, 0x75, 0x96, 0x56, 0xdd, 0x53,
        0x61, 0xd1, 0xc6, 0xf0, 0xe3, 0xee, 0xbd, 0x9a, 0xb5, 0xab, 0x06, 0x01, 0xa1, 0x26, 0xca,
        0x53, 0x0c, 0x8e, 0x48, 0xd5, 0xb8, 0x88, 0x82, 0xc7, 0x34, 0xc3, 0x89, 0x35, 0x05, 0x01,
        0x8f, 0x15, 0x1b, 0xa4, 0xde, 0x20, 0x4e, 0x3c, 0x9c, 0xb4, 0x99, 0xdd, 0xf9, 0x6f, 0x48,
        0xf1, 0x01, 0x09, 0x0a, 0xda, 0xc0, 0xfc, 0xa4, 0x82, 0x2e, 0x8e, 0x71, 0x92, 0x63, 0xe6,
        0x76, 0x20, 0xec, 0x00, 0x02, 0xde, 0xc3, 0xc8, 0xca, 0xe0, 0x71, 0x48, 0x23, 0x94, 0xf1,
        0xdc, 0x4d, 0xe1, 0x1e, 0x7f, 0xb6, 0xe7, 0xd7, 0x37, 0xc5, 0x36, 0x76, 0x4c, 0x60, 0x9f,
        0xa1, 0x6a, 0xa6, 0x4a, 0x8c, 0x90, 0xad, 0x00, 0x00, 0x02, 0x05, 0x91, 0xe3, 0xb1, 0x48,
        0x52, 0x7a, 0x47, 0x93, 0xbf, 0x50, 0x4a, 0x46, 0x08, 0x81, 0xda, 0x0a, 0x00, 0x34, 0x00,
        0x01, 0xa1, 0x9c, 0x34, 0x5a, 0xb9, 0x86, 0x66, 0x79, 0xb0, 0x01, 0xd7, 0xd2, 0x13, 0x8d,
        0x88, 0xa1, 0x28, 0x71, 0x5c, 0xac, 0x33, 0xc6, 0x40, 0x6d, 0xa2, 0xdb, 0x0b, 0x03, 0x7b,
        0x89, 0xcc, 0x7a, 0x05, 0x61, 0x30, 0x33, 0x7a, 0x50, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x02,
        0x00, 0x01, 0x01, 0x00, 0x0c, 0x44, 0x65, 0x62, 0x61, 0x74, 0x65, 0x73, 0x20, 0x70, 0x61,
        0x67, 0x65, 0x01, 0xff, 0xff, 0xff, 0xff, 0x0f,
    ];

    // Real CIDv1 (raw codec, base32 multibase) production payload + CID
    // ("Create payout") — the dominant real-world shape found once the
    // 2026-07-20 backlog audit ran: unlike the CIDv0 fixtures above, this
    // codec's digest is a direct sha2-256 of the raw bytes, no dag-pb/UnixFS
    // wrapping involved.
    pub(crate) const GOLDEN_CIDV1_RAW_CID: &str =
        "bafkreiaexy7lean2gu56wdy6wonpfcvmrxqduqwtx3bsgionxx3v4ugc5y";
    pub(crate) const GOLDEN_CIDV1_RAW_BYTES: &[u8] = &[
        0x47, 0x52, 0x43, 0x32, 0x00, 0x06, 0x09, 0x67, 0xe3, 0xf1, 0x64, 0x46, 0xe1, 0xbe, 0x68,
        0xd9, 0x76, 0xb3, 0xbe, 0x38, 0x39, 0x0d, 0x43, 0x72, 0x65, 0x61, 0x74, 0x65, 0x20, 0x70,
        0x61, 0x79, 0x6f, 0x75, 0x74, 0x01, 0x07, 0x84, 0x28, 0x62, 0xd2, 0xc3, 0x65, 0x4c, 0x03,
        0x24, 0xa0, 0x7b, 0xc7, 0xcc, 0xe1, 0xa4, 0xf0, 0x8f, 0xd7, 0x81, 0x97, 0x96, 0xa6, 0x06,
        0x02, 0xa1, 0x26, 0xca, 0x53, 0x0c, 0x8e, 0x48, 0xd5, 0xb8, 0x88, 0x82, 0xc7, 0x34, 0xc3,
        0x89, 0x35, 0x05, 0x97, 0x28, 0xa6, 0xaa, 0xd7, 0xd7, 0x5a, 0x48, 0x29, 0xbb, 0x41, 0x18,
        0xad, 0x28, 0xb6, 0xc0, 0x04, 0x03, 0xfd, 0xda, 0xca, 0xae, 0x85, 0x13, 0x8a, 0x43, 0xec,
        0x1a, 0x50, 0xff, 0x71, 0x56, 0x4d, 0x42, 0x8f, 0x15, 0x1b, 0xa4, 0xde, 0x20, 0x4e, 0x3c,
        0x9c, 0xb4, 0x99, 0xdd, 0xf9, 0x6f, 0x48, 0xf1, 0x1b, 0x59, 0x5a, 0x8b, 0x81, 0xfc, 0x25,
        0x85, 0x6a, 0x9b, 0x50, 0x3e, 0x3e, 0x99, 0x33, 0x31, 0x01, 0x09, 0x0a, 0xda, 0xc0, 0xfc,
        0xa4, 0x82, 0x2e, 0x8e, 0x71, 0x92, 0x63, 0xe6, 0x76, 0x20, 0xec, 0x00, 0x05, 0x4e, 0x6d,
        0x70, 0xbd, 0xe6, 0x72, 0xf7, 0xac, 0x25, 0x69, 0x39, 0xeb, 0xa8, 0x2c, 0x26, 0xc3, 0x40,
        0x70, 0x49, 0xae, 0xd8, 0x01, 0x48, 0x03, 0x9d, 0xfd, 0x52, 0x8d, 0xaf, 0x89, 0x80, 0x81,
        0xbf, 0x99, 0xb1, 0x79, 0xf2, 0xcf, 0x4b, 0x1a, 0xb1, 0xa3, 0xcd, 0x05, 0xe8, 0x03, 0x24,
        0xc6, 0xf5, 0x13, 0x2d, 0xeb, 0x10, 0x2d, 0x64, 0x55, 0x30, 0x49, 0xf1, 0xe9, 0xcb, 0x66,
        0x2f, 0x50, 0x33, 0x0f, 0x72, 0xc7, 0xd0, 0xee, 0x4e, 0xd5, 0xbf, 0xa7, 0x34, 0xa1, 0x4f,
        0xc7, 0x85, 0x32, 0x00, 0x00, 0x04, 0x05, 0x2f, 0xb9, 0xca, 0x0d, 0x0d, 0x50, 0x43, 0x83,
        0xa4, 0xfc, 0x29, 0xbf, 0x0a, 0xb4, 0x04, 0xe6, 0x00, 0x10, 0x00, 0x01, 0xbf, 0x99, 0xb1,
        0x79, 0xf2, 0xcf, 0x4b, 0x1a, 0xb1, 0xa3, 0xcd, 0x05, 0xe8, 0x03, 0x24, 0xc6, 0xff, 0xff,
        0xff, 0xff, 0x0f, 0x01, 0xbf, 0x99, 0xb1, 0x79, 0xf2, 0xcf, 0x4b, 0x1a, 0xb1, 0xa3, 0xcd,
        0x05, 0xe8, 0x03, 0x24, 0xc6, 0x02, 0x00, 0x11, 0x50, 0x61, 0x79, 0x6f, 0x75, 0x74, 0x20,
        0x74, 0x6f, 0x20, 0x4e, 0x69, 0x6b, 0x20, 0x39, 0x30, 0x32, 0x01, 0x01, 0x00, 0x00, 0x14,
        0x00, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x05, 0x8e, 0x3e, 0xc7, 0x70, 0x25, 0x05, 0x4d, 0x20,
        0x8a, 0xf4, 0x2e, 0x91, 0x96, 0xb9, 0x94, 0xe7, 0x01, 0x10, 0x02, 0x03, 0x01, 0x1a, 0xd1,
        0x35, 0xba, 0xa9, 0x4f, 0x97, 0xaf, 0x82, 0x2e, 0x22, 0x7d, 0x6d, 0x49, 0xc6, 0xff, 0xff,
        0xff, 0xff, 0x0f, 0x05, 0xd0, 0x2e, 0xe6, 0xf6, 0xe0, 0x22, 0x49, 0xa0, 0xa9, 0xa2, 0x69,
        0x03, 0x68, 0x8b, 0x2a, 0x78, 0x02, 0x10, 0x02, 0x04, 0x22, 0xd5, 0xc6, 0xa0, 0x8d, 0xa4,
        0x4c, 0x96, 0xb6, 0xa1, 0x20, 0xca, 0x84, 0x05, 0x55, 0x37, 0xff, 0xff, 0xff, 0xff, 0x0f,
    ];

    // Second real CIDv1 raw-codec fixture ("test 2") — different length,
    // independent confirmation of the raw-codec path.
    pub(crate) const GOLDEN_CIDV1_RAW_CID_2: &str =
        "bafkreiaez5vauo6q2wk5pqun3itvkq6clvjfxjtcdcgawtknodo6ouzihu";
    pub(crate) const GOLDEN_CIDV1_RAW_BYTES_2: &[u8] = &[
        0x47, 0x52, 0x43, 0x32, 0x00, 0xd7, 0x72, 0x72, 0xcd, 0xa1, 0x8e, 0x46, 0x4e, 0x8a, 0x06,
        0x7a, 0x54, 0x23, 0xa5, 0x4c, 0x73, 0x06, 0x74, 0x65, 0x73, 0x74, 0x20, 0x32, 0x01, 0x1d,
        0x01, 0x46, 0x98, 0xe7, 0x4b, 0xb2, 0xd9, 0x6f, 0xdb, 0x77, 0xdd, 0x08, 0x6f, 0x3a, 0x03,
        0xd0, 0x8d, 0xf4, 0xd8, 0xf1, 0xa9, 0xa5, 0x06, 0x03, 0xa1, 0x26, 0xca, 0x53, 0x0c, 0x8e,
        0x48, 0xd5, 0xb8, 0x88, 0x82, 0xc7, 0x34, 0xc3, 0x89, 0x35, 0x05, 0x36, 0x1d, 0xe8, 0xfa,
        0xd0, 0xe4, 0x44, 0xdc, 0xa2, 0xd2, 0x69, 0x39, 0x88, 0x45, 0x80, 0x98, 0x04, 0xcd, 0xbc,
        0x35, 0xe8, 0xd0, 0x22, 0x41, 0xd1, 0x8d, 0xa2, 0xa5, 0x4a, 0x96, 0x87, 0x0b, 0xe9, 0x04,
        0x02, 0x6d, 0x29, 0xd5, 0x78, 0x49, 0xbb, 0x49, 0x59, 0xba, 0xf7, 0x2c, 0xc6, 0x96, 0xb1,
        0x67, 0x1a, 0x8f, 0x15, 0x1b, 0xa4, 0xde, 0x20, 0x4e, 0x3c, 0x9c, 0xb4, 0x99, 0xdd, 0xf9,
        0x6f, 0x48, 0xf1, 0x01, 0x09, 0x0a, 0xda, 0xc0, 0xfc, 0xa4, 0x82, 0x2e, 0x8e, 0x71, 0x92,
        0x63, 0xe6, 0x76, 0x20, 0xec, 0x00, 0x04, 0xcd, 0xbc, 0x35, 0xe8, 0xd0, 0x22, 0x41, 0xd1,
        0x8d, 0xa2, 0xa5, 0x4a, 0x96, 0x87, 0x0b, 0xe9, 0xa3, 0x28, 0x8c, 0x22, 0xa0, 0x56, 0x4f,
        0x6f, 0xb4, 0x09, 0xfb, 0xcc, 0xcb, 0x2c, 0x11, 0x8c, 0x80, 0x8a, 0x04, 0xce, 0xb2, 0x1c,
        0x4d, 0x88, 0x8a, 0xd1, 0x2e, 0x24, 0x06, 0x13, 0xe5, 0xca, 0xbc, 0x63, 0xe4, 0x2b, 0xc0,
        0xde, 0x4c, 0x2c, 0xb2, 0x79, 0x3a, 0x98, 0x4a, 0xfe, 0xc0, 0x20, 0x00, 0x00, 0x04, 0x05,
        0xe7, 0x51, 0xfa, 0x32, 0xdd, 0x10, 0x41, 0x49, 0xb3, 0x7a, 0x4f, 0xe7, 0xe2, 0x88, 0x38,
        0xbb, 0x00, 0x30, 0x00, 0x01, 0xee, 0x31, 0x41, 0x93, 0x0f, 0xe0, 0x47, 0x62, 0xb3, 0x69,
        0x81, 0x8c, 0xbe, 0x05, 0x99, 0x03, 0x05, 0x61, 0x30, 0x39, 0x4a, 0x52, 0xff, 0xff, 0xff,
        0xff, 0x0f, 0x05, 0x84, 0xce, 0xf0, 0x14, 0x54, 0x72, 0x4d, 0xcb, 0x97, 0xe1, 0xb1, 0xd5,
        0x72, 0x91, 0xcf, 0x88, 0x01, 0x30, 0x00, 0x02, 0x84, 0xfc, 0xe8, 0x27, 0x09, 0x70, 0x43,
        0xb7, 0x82, 0xa8, 0x20, 0x70, 0x29, 0x1e, 0xbf, 0x27, 0x05, 0x61, 0x30, 0x31, 0x43, 0x43,
        0xff, 0xff, 0xff, 0xff, 0x0f, 0x02, 0x00, 0x01, 0x01, 0x00, 0x0e, 0x54, 0x65, 0x73, 0x74,
        0x20, 0x64, 0x65, 0x63, 0x69, 0x6d, 0x61, 0x6c, 0x20, 0x32, 0x01, 0xff, 0xff, 0xff, 0xff,
        0x0f, 0x02, 0x03, 0x01, 0x02, 0x01, 0x03, 0x00, 0xe4, 0x0f, 0x00, 0x02, 0x03, 0x00, 0xe4,
        0x0f, 0x00, 0xff, 0xff, 0xff, 0xff, 0x0f,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixtures::*;

    #[test]
    fn verifies_known_good_content() {
        assert_eq!(
            verify(GOLDEN_SMALL_CID, GOLDEN_SMALL_BYTES),
            Verification::Verified
        );
    }

    #[test]
    fn verifies_a_multi_byte_varint_length_field() {
        assert_eq!(
            verify(GOLDEN_LARGE_CID, GOLDEN_LARGE_BYTES),
            Verification::Verified
        );
    }

    #[test]
    fn rejects_mismatched_content() {
        let mut tampered = GOLDEN_SMALL_BYTES.to_vec();
        tampered.push(0x00);
        assert_eq!(verify(GOLDEN_SMALL_CID, &tampered), Verification::Mismatch);
    }

    #[test]
    fn rejects_truncated_content() {
        let truncated = &GOLDEN_SMALL_BYTES[..GOLDEN_SMALL_BYTES.len() - 1];
        assert_eq!(verify(GOLDEN_SMALL_CID, truncated), Verification::Mismatch);
    }

    #[test]
    fn reports_unrecognized_hash_function_as_unsupported() {
        // CIDv1, raw codec, but blake2b-256 (mh code 0xb220) instead of
        // sha2-256 — a shape this module deliberately doesn't attempt to
        // verify rather than mis-hash.
        let mut bytes = vec![1u8, CODEC_RAW as u8];
        write_varint(&mut bytes, 0xb220);
        bytes.push(32); // digest length
        bytes.extend_from_slice(&[0u8; 32]);
        let cid = format!("b{}", base32_encode_nopad(&bytes));

        assert_eq!(verify(&cid, b"anything"), Verification::Unsupported);
    }

    #[test]
    fn reports_non_base32_cidv1_as_unsupported() {
        // Doesn't start with 'b' (base32) and isn't valid base58 either
        // (contains characters base58 excludes) — genuinely unparseable,
        // not a shape we quietly mis-verify.
        let result = verify("not-a-real-cid-0O0Il", b"anything");
        assert_eq!(result, Verification::Unsupported);
    }

    #[test]
    fn verifies_uppercase_multibase_prefix() {
        // 'B' (uppercase) is the multibase code for base32-upper — equally
        // valid per the multibase spec, just far rarer in practice than
        // lowercase 'b'. Uppercasing everything after the prefix on a known
        // real CID should still verify.
        let uppercased = format!("B{}", GOLDEN_CIDV1_RAW_CID[1..].to_ascii_uppercase());
        assert_eq!(
            verify(&uppercased, GOLDEN_CIDV1_RAW_BYTES),
            Verification::Verified
        );
    }

    #[test]
    fn rejects_cidv1_with_trailing_bytes_after_the_digest() {
        // A well-formed header + digest followed by extra bytes is not a
        // canonical CIDv1 multihash — reject rather than silently ignoring
        // the trailing bytes.
        let mut bytes = vec![1u8, CODEC_RAW as u8, MULTIHASH_SHA2_256 as u8, 32];
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.push(0xFF); // trailing garbage past the digest
        let cid = format!("b{}", base32_encode_nopad(&bytes));

        assert_eq!(verify(&cid, b"anything"), Verification::Unsupported);
    }

    #[test]
    fn reports_oversized_content_as_unsupported() {
        let big = vec![0u8; 256 * 1024 + 1];
        assert_eq!(verify(GOLDEN_SMALL_CID, &big), Verification::Unsupported);
    }

    #[test]
    fn strips_ipfs_prefix() {
        let uri = format!("ipfs://{GOLDEN_SMALL_CID}");
        // Wrong bytes on purpose — just checking the prefix is stripped and
        // we get a real verdict (Mismatch), not silently treated as
        // Unsupported due to a malformed decode from the un-stripped prefix.
        assert_eq!(verify(&uri, b"wrong"), Verification::Mismatch);
    }

    #[test]
    fn verifies_known_good_cidv1_raw_content() {
        assert_eq!(
            verify(GOLDEN_CIDV1_RAW_CID, GOLDEN_CIDV1_RAW_BYTES),
            Verification::Verified
        );
        assert_eq!(
            verify(GOLDEN_CIDV1_RAW_CID_2, GOLDEN_CIDV1_RAW_BYTES_2),
            Verification::Verified
        );
    }

    #[test]
    fn rejects_mismatched_cidv1_raw_content() {
        let mut tampered = GOLDEN_CIDV1_RAW_BYTES.to_vec();
        tampered.push(0x00);
        assert_eq!(
            verify(GOLDEN_CIDV1_RAW_CID, &tampered),
            Verification::Mismatch
        );
    }

    #[test]
    fn rejects_content_swapped_between_two_real_cidv1_uris() {
        // GOLDEN_CIDV1_RAW_BYTES_2 is real, valid content for a *different*
        // CID — make sure verify() doesn't just check "is this decodable",
        // it checks "does this match *this* CID".
        assert_eq!(
            verify(GOLDEN_CIDV1_RAW_CID, GOLDEN_CIDV1_RAW_BYTES_2),
            Verification::Mismatch
        );
    }

    #[test]
    fn cidv1_raw_has_no_single_chunk_size_limit() {
        // Unlike dag-pb, a raw-codec CID is never chunked by construction —
        // large content should still be checked directly, not bailed out to
        // Unsupported the way an oversized dag-pb payload is.
        let big = vec![0xABu8; 256 * 1024 + 1];
        let digest = Sha256::digest(&big);
        let cid = cidv1_raw_string(&digest);
        assert_eq!(verify(&cid, &big), Verification::Verified);
    }

    /// Build a CIDv1 raw-codec string for a given sha2-256 digest, for use
    /// as a test fixture (encodes: version=1, codec=raw, mh-code=sha2-256,
    /// mh-len=32, digest).
    fn cidv1_raw_string(digest: &[u8]) -> String {
        let mut bytes = vec![
            1u8,
            CODEC_RAW as u8,
            MULTIHASH_SHA2_256 as u8,
            digest.len() as u8,
        ];
        bytes.extend_from_slice(digest);
        format!("b{}", base32_encode_nopad(&bytes))
    }

    fn base32_encode_nopad(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
        let mut bits: u64 = 0;
        let mut bit_count: u32 = 0;
        let mut out = String::new();
        for &b in bytes {
            bits = (bits << 8) | b as u64;
            bit_count += 8;
            while bit_count >= 5 {
                bit_count -= 5;
                out.push(ALPHABET[((bits >> bit_count) & 0x1f) as usize] as char);
            }
        }
        if bit_count > 0 {
            out.push(ALPHABET[((bits << (5 - bit_count)) & 0x1f) as usize] as char);
        }
        out
    }
}
