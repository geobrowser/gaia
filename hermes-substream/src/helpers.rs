use substreams::Hex;

/// Returns the hex representation of the address in lowercase with 0x prefix
pub fn format_hex(address: &[u8]) -> String {
    format!("0x{}", Hex(address).to_string())
}
