//! Minimal AWS Signature Version 4 for S3 (header auth, path-style, no query string).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

pub struct Signed {
    pub signature: String,
    pub signed_headers: String,
}

/// Compute the SigV4 signature. `headers` are lowercase-named `(name, value)` pairs that will
/// be signed; they must be sorted by name. `canonical_uri` is the already-encoded URL path.
#[allow(clippy::too_many_arguments)]
pub fn sign(
    method: &str,
    canonical_uri: &str,
    headers: &[(&str, &str)],
    payload_hash: &str,
    secret_key: &str,
    region: &str,
    amz_date: &str,
) -> Signed {
    let date = &amz_date[..8];
    let canonical_headers: String = headers
        .iter()
        .map(|(k, v)| format!("{k}:{}\n", v.trim()))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac(format!("AWS4{secret_key}").as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    Signed {
        signature: hex::encode(hmac(&k_signing, &string_to_sign)),
        signed_headers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The "GET Object" example from the AWS Signature Version 4 documentation for S3.
    #[test]
    fn aws_documented_get_object_vector() {
        let empty = sha256_hex(b"");
        assert_eq!(
            empty,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let s = sign(
            "GET",
            "/test.txt",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", &empty),
                ("x-amz-date", "20130524T000000Z"),
            ],
            &empty,
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "20130524T000000Z",
        );
        assert_eq!(
            s.signature,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        assert_eq!(
            s.signed_headers,
            "host;range;x-amz-content-sha256;x-amz-date"
        );
    }
}
