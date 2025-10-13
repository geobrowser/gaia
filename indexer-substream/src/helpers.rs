use substreams::{Hex, store::{StoreGet, StoreGetString}};

/// This function will return the hex representation of the address in lowercase
pub fn format_hex(address: &[u8]) -> String {
    format!("0x{}", Hex(address).to_string())
}

/// Validates if a plugin address is authorized for a DAO based on governance type
/// Returns the DAO address if the plugin is valid, None if invalid
pub fn validate_plugin_for_dao(
    plugin_address: &str,
    get_dao_governance_type: &StoreGetString,
    get_dao_plugins: &StoreGetString,
    get_plugin_to_dao: &StoreGetString,
) -> Option<String> {
    substreams::log::info!("Validating plugin: {}", plugin_address);
    
    let dao_address = match get_plugin_to_dao.get_last(&format!("plugin:{}", plugin_address)) {
        Some(dao) => {
            substreams::log::info!("Found DAO {} for plugin {}", dao, plugin_address);
            dao
        },
        None => {
            substreams::log::info!("Plugin {} not found in store - filtering out", plugin_address);
            return None;
        }
    };
    
    let dao_type = get_dao_governance_type.get_last(&format!("dao:type:{}", dao_address));
    
    let is_valid = match dao_type.as_ref().map(|s| s.as_str()) {
        Some("personal") => {
            substreams::log::info!("DAO {} has governance type: personal", dao_address);
            let personal_key = format!("dao:personal_admin:{}", dao_address);
            let result = get_dao_plugins.get_last(&personal_key) == Some(plugin_address.to_string());
            substreams::log::info!("Personal validation for plugin {} against DAO {}: {}", plugin_address, dao_address, result);
            result
        },
        Some("governance") => {
            substreams::log::info!("DAO {} has governance type: governance", dao_address);
            let member_key = format!("dao:member_access:{}", dao_address);
            let voting_key = format!("dao:voting:{}", dao_address);
            let member_match = get_dao_plugins.get_last(&member_key) == Some(plugin_address.to_string());
            let voting_match = get_dao_plugins.get_last(&voting_key) == Some(plugin_address.to_string());
            let result = member_match || voting_match;
            substreams::log::info!("Governance validation for plugin {} against DAO {}: member={}, voting={}, result={}", 
                plugin_address, dao_address, member_match, voting_match, result);
            result
        },
        Some(unknown_type) => {
            substreams::log::info!("Unknown governance type '{}' for DAO {} - filtering out", unknown_type, dao_address);
            false
        },
        None => {
            substreams::log::info!("DAO {} type not found in store - this is likely first plugin registration, allowing", dao_address);
            // For first-time plugin registration, check if the plugin is actually registered
            let space_key = format!("dao:space:{}", dao_address);
            let member_key = format!("dao:member_access:{}", dao_address);
            let voting_key = format!("dao:voting:{}", dao_address);
            let personal_key = format!("dao:personal_admin:{}", dao_address);
            
            let is_space = get_dao_plugins.get_last(&space_key) == Some(plugin_address.to_string());
            let is_member = get_dao_plugins.get_last(&member_key) == Some(plugin_address.to_string());
            let is_voting = get_dao_plugins.get_last(&voting_key) == Some(plugin_address.to_string());
            let is_personal = get_dao_plugins.get_last(&personal_key) == Some(plugin_address.to_string());
            
            let result = is_space || is_member || is_voting || is_personal;
            substreams::log::info!("First-time validation for plugin {} against DAO {}: space={}, member={}, voting={}, personal={}, result={}", 
                plugin_address, dao_address, is_space, is_member, is_voting, is_personal, result);
            result
        }
    };
    
    if is_valid {
        substreams::log::info!("Plugin {} is valid for DAO {}", plugin_address, dao_address);
        Some(dao_address)
    } else {
        let dao_type_str = dao_type.as_deref().unwrap_or("none");
        substreams::log::info!("Plugin {} is NOT valid for DAO {} with type '{}' - filtering out", plugin_address, dao_address, dao_type_str);
        None
    }
}
