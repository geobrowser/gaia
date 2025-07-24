use std::io;

/// Decompresses zstd-compressed data from a byte slice
pub fn decompress_bytes(compressed_data: &[u8]) -> io::Result<Vec<u8>> {
    zstd::decode_all(compressed_data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Decompresses zstd-compressed data and converts it to a UTF-8 string
pub fn decompress_to_string(compressed_data: &[u8]) -> io::Result<String> {
    let decompressed_bytes = decompress_bytes(compressed_data)?;
    String::from_utf8(decompressed_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid UTF-8: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_decompress_ops_json_zst() {
        // Read the compressed file
        let compressed_data =
            fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file");

        // Decompress the data
        let decompressed_data =
            decompress_bytes(&compressed_data).expect("Failed to decompress ops.json.zst");

        // Verify we got some data back
        assert!(
            !decompressed_data.is_empty(),
            "Decompressed data should not be empty"
        );

        // Verify it's valid UTF-8 (since it should be JSON)
        let json_str =
            String::from_utf8(decompressed_data).expect("Decompressed data should be valid UTF-8");

        // Verify it looks like JSON (starts with { or [)
        let trimmed = json_str.trim();
        assert!(
            trimmed.starts_with('{') || trimmed.starts_with('['),
            "Decompressed data should be valid JSON"
        );
    }

    #[test]
    fn test_decompress_ops_json_zst_to_string() {
        // Read the compressed file
        let compressed_data =
            fs::read("data/ops.json.zst").expect("Failed to read ops.json.zst file");

        // Decompress the data to string
        let json_string = decompress_to_string(&compressed_data)
            .expect("Failed to decompress ops.json.zst to string");

        // Verify we got some data back
        assert!(
            !json_string.is_empty(),
            "Decompressed string should not be empty"
        );

        // Verify it looks like JSON (starts with { or [)
        let trimmed = json_string.trim();
        assert!(
            trimmed.starts_with('{') || trimmed.starts_with('['),
            "Decompressed string should be valid JSON"
        );
    }
}
