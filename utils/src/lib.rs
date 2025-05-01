use sha3::{Digest, Keccak256};

pub fn checksum_address(address: impl Into<String>) -> String {
    let input_address = address.into().to_lowercase().replace("0x", "");

    let mut hasher = Keccak256::new();
    hasher.update(input_address.as_bytes());
    let hash = hasher.finalize();

    let mut address_chars: Vec<char> = input_address.chars().collect();

    for i in (0..40).step_by(2) {
        if (hash[i / 2] >> 4) >= 8 && address_chars[i].is_ascii() {
            address_chars[i] = address_chars[i].to_ascii_uppercase();
        }
        if (hash[i / 2] & 0x0f) >= 8 && address_chars[i + 1].is_ascii() {
            address_chars[i + 1] = address_chars[i + 1].to_ascii_uppercase();
        }
    }

    format!("0x{}", address_chars.iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_address() {
        assert_eq!(
            checksum_address("0x5a0b54d5dc17e0aadc383d2db43b0a0d3e029c4c"),
            "0x5A0b54D5dc17e0AadC383d2db43B0a0D3E029c4c"
        );
        assert_eq!(
            checksum_address("0x5A0b54D5dc17e0AadC383d2db43B0a0D3E029c4c"),
            "0x5A0b54D5dc17e0AadC383d2db43B0a0D3E029c4c"
        );
        assert_eq!(
            checksum_address("0xfb6916095ca1df60bb79ce92ce3ea74c37c5d359"),
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359"
        );
    }
}
