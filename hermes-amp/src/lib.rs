//! Hermes Amp mappings and manifests.

/// Space Registry proxy contract address (ZC16 testnet).
pub const SPACE_REGISTRY_ADDRESS_HEX: &str = "0xb01683b2f0d38d43fcd4d9aab980166988924132";

/// Derived dataset manifest that maps Hermes actions from Amp logs.
pub const HERMES_ACTIONS_MANIFEST_JSON: &str =
    include_str!("../manifests/hermes-actions.json");
