pub mod id;
pub mod network_ids;

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

pub struct SpacesBlocklist<'a> {
    pub space_ids: Vec<&'a str>,
    pub dao_addresses: Vec<&'a str>,
    pub space_plugin_addresses: Vec<&'a str>,
    pub main_voting_plugin_addresses: Vec<&'a str>,
    pub member_access_plugin_address: Vec<&'a str>,
}

pub fn get_blocklist() -> SpacesBlocklist<'static> {
    SpacesBlocklist {
        space_ids: vec!["Q5YFEacgaHtXE9Kub9AEkA"],
        dao_addresses: vec!["0x22238cd64d914583f06223adfe9cddf9b45d1971"],
        main_voting_plugin_addresses: vec!["0x8Cb274d585393acd5277EC2B29ab56F2B604E4f0"],
        member_access_plugin_address: vec!["0x27e73AD87612098F9F7c2F456E9f4803DAcd899B"],
        space_plugin_addresses: vec!["0x07801e72a8a722969663440385b906e1b073c948"],
    }
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
