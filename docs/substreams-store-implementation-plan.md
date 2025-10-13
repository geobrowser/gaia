# Substreams Store Implementation Plan for DAO Plugin Governance Type Enforcement

## Overview
Implement a Substreams Store that maintains DAO→Plugin mappings, preventing mixing of governance types (Personal vs Standard/Public spaces). **All plugin validation is centralized in the `geo_out` function for clean architecture and consistent enforcement.**

## Store Design

### 1. Core Store Modules

#### Store: `store_dao_governance_type`
**Purpose**: Track the governance type of each DAO (simple tracking, no validation)
- **Key Format**: `dao:type:{dao_address}`
- **Value**: `"governance"` | `"personal"`
- **Update Policy**: `set`

#### Store: `store_dao_plugins`
**Purpose**: Track all plugin addresses for each DAO
- **Key Formats**:
  - `dao:space:{dao_address}` → space_address
  - `dao:voting:{dao_address}` → main_voting_address  
  - `dao:member_access:{dao_address}` → member_access_address
  - `dao:personal_admin:{dao_address}` → personal_admin_address
- **Update Policy**: `set`

#### Store: `store_plugin_to_dao`
**Purpose**: Reverse mapping for quick plugin validation
- **Key Format**: `plugin:{plugin_address}` → dao_address
- **Update Policy**: `set`

### 2. Store Population Logic (Simple, No Validation)

#### Handler: `store_dao_governance_type`
```rust
#[substreams::handlers::store]
fn store_dao_governance_type(
    governance_plugins: GeoGovernancePluginsCreated,
    personal_plugins: GeoPersonalSpaceAdminPluginsCreated,
    store: StoreSetString,
) {
    for plugin in governance_plugins.plugins {
        let dao_type_key = format!("dao:type:{}", plugin.dao_address);
        store.set(0, dao_type_key, &"governance".to_string());
    }
    
    for plugin in personal_plugins.plugins {
        let dao_type_key = format!("dao:type:{}", plugin.dao_address);
        store.set(0, dao_type_key, &"personal".to_string());
    }
}
```

#### Handler: `store_dao_plugins`
```rust
#[substreams::handlers::store]
fn store_dao_plugins(
    spaces: GeoSpacesCreated,
    governance_plugins: GeoGovernancePluginsCreated,
    personal_plugins: GeoPersonalSpaceAdminPluginsCreated,
    store: StoreSetString,
) {
    // Store space plugins (common to both types)
    for space in spaces.spaces {
        let space_key = format!("dao:space:{}", space.dao_address);
        store.set(0, space_key, &space.space_address);
    }
    
    // Store governance plugins
    for plugin in governance_plugins.plugins {
        let voting_key = format!("dao:voting:{}", plugin.dao_address);
        let member_key = format!("dao:member_access:{}", plugin.dao_address);
        store.set(0, voting_key, &plugin.main_voting_address);
        store.set(0, member_key, &plugin.member_access_address);
    }
    
    // Store personal admin plugins
    for plugin in personal_plugins.plugins {
        let personal_admin_key = format!("dao:personal_admin:{}", plugin.dao_address);
        store.set(0, personal_admin_key, &plugin.personal_admin_address);
    }
}
```

#### Handler: `store_plugin_to_dao`
```rust
#[substreams::handlers::store]
fn store_plugin_to_dao(
    spaces: GeoSpacesCreated,
    governance_plugins: GeoGovernancePluginsCreated,
    personal_plugins: GeoPersonalSpaceAdminPluginsCreated,
    store: StoreSetString,
) {
    // Store space plugin mappings
    for space in spaces.spaces {
        let plugin_key = format!("plugin:{}", space.space_address);
        store.set(0, plugin_key, &space.dao_address);
    }
    
    // Store governance plugin mappings
    for plugin in governance_plugins.plugins {
        let voting_plugin_key = format!("plugin:{}", plugin.main_voting_address);
        let member_plugin_key = format!("plugin:{}", plugin.member_access_address);
        store.set(0, voting_plugin_key, &plugin.dao_address);
        store.set(0, member_plugin_key, &plugin.dao_address);
    }
    
    // Store personal admin plugin mappings
    for plugin in personal_plugins.plugins {
        let plugin_key = format!("plugin:{}", plugin.personal_admin_address);
        store.set(0, plugin_key, &plugin.dao_address);
    }
}
```

### 3. Centralized Event Validation in `geo_out`

#### Progressive Validation Helper Function
```rust
pub fn validate_plugin_for_dao(
    plugin_address: &str,
    get_dao_governance_type: &StoreGetString,
    get_dao_plugins: &StoreGetString,
    get_plugin_to_dao: &StoreGetString,
) -> Option<String> {
    // 1. Look up DAO address from plugin
    let dao_address = get_plugin_to_dao.get_last(&format!("plugin:{}", plugin_address))?;
    
    // 2. Check if DAO has established governance type
    let dao_type = get_dao_governance_type.get_last(&format!("dao:type:{}", dao_address));
    
    let is_valid = match dao_type.as_ref().map(|s| s.as_str()) {
        // 3a. Strict validation against established governance type
        Some("personal") => {
            let personal_key = format!("dao:personal_admin:{}", dao_address);
            get_dao_plugins.get_last(&personal_key) == Some(plugin_address.to_string())
        },
        Some("governance") => {
            let member_key = format!("dao:member_access:{}", dao_address);
            let voting_key = format!("dao:voting:{}", dao_address);
            get_dao_plugins.get_last(&member_key) == Some(plugin_address.to_string()) ||
            get_dao_plugins.get_last(&voting_key) == Some(plugin_address.to_string())
        },
        // 3b. First-time validation - allow any registered plugin
        None => {
            let space_key = format!("dao:space:{}", dao_address);
            let member_key = format!("dao:member_access:{}", dao_address);
            let voting_key = format!("dao:voting:{}", dao_address);
            let personal_key = format!("dao:personal_admin:{}", dao_address);
            
            get_dao_plugins.get_last(&space_key) == Some(plugin_address.to_string()) ||
            get_dao_plugins.get_last(&member_key) == Some(plugin_address.to_string()) ||
            get_dao_plugins.get_last(&voting_key) == Some(plugin_address.to_string()) ||
            get_dao_plugins.get_last(&personal_key) == Some(plugin_address.to_string())
        },
        _ => false
    };
    
    if is_valid { Some(dao_address) } else { None }
}
```

#### Main Validation in `geo_out`
```rust
#[substreams::handlers::map]
fn geo_out(
    // ... all map inputs ...
    get_dao_governance_type: StoreGetString,
    get_dao_plugins: StoreGetString,
    get_plugin_to_dao: StoreGetString,
) -> Result<GeoOutput, substreams::errors::Error> {
    // Plugin creation events are NEVER validated - they establish legitimate plugins
    let spaces_created = spaces_created.spaces;
    let governance_plugins_created = governance_plugins_created.plugins;
    let personal_admin_plugins_created = personal_admin_plugins_created.plugins;
    
    // Filter operational plugin-based events through progressive validation
    let votes_cast = votes_cast.votes.into_iter()
        .filter(|vote| validate_plugin_for_dao(&vote.plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let members_added = members_added.members.into_iter()
        .filter(|member| validate_plugin_for_dao(&member.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let edits_published = edits_published.edits.into_iter()
        .filter(|edit| validate_plugin_for_dao(&edit.space_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    // Filter all other operational events with progressive validation
    let proposals_executed = proposals_executed.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let members_removed = members_removed.members.into_iter()
        .filter(|member| validate_plugin_for_dao(&member.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let editors_added = editors_added.editors.into_iter()
        .filter(|editor| validate_plugin_for_dao(&editor.space_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let editors_removed = editors_removed.editors.into_iter()
        .filter(|editor| validate_plugin_for_dao(&editor.space_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    // Filter all proposal creation events
    let publish_edits_proposals_created = publish_edits_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let add_member_proposals_created = add_member_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let remove_member_proposals_created = remove_member_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let add_editor_proposals_created = add_editor_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let remove_editor_proposals_created = remove_editor_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let add_subspace_proposals_created = add_subspace_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    let remove_subspace_proposals_created = remove_subspace_proposals_created.proposals.into_iter()
        .filter(|proposal| validate_plugin_for_dao(&proposal.main_voting_plugin_address, &get_dao_governance_type, &get_dao_plugins, &get_plugin_to_dao).is_some())
        .collect();
    
    Ok(GeoOutput {
        // Plugin creation events - always included (never validated)
        spaces_created,
        governance_plugins_created,
        personal_admin_plugins_created,
        
        // Operational events - validated with progressive validation
        votes_cast,
        edits_published,
        members_added,
        members_removed,
        editors_added,
        editors_removed,
        proposals_executed,
        
        // Proposal creation events - validated
        publish_edits_proposals_created,
        add_member_proposals_created,
        remove_member_proposals_created,
        add_editor_proposals_created,
        remove_editor_proposals_created,
        add_subspace_proposals_created,
        remove_subspace_proposals_created,
        
        // Non-plugin events - never validated (always included)
        successor_spaces_created: successor_spaces_created.spaces,
        subspaces_added: subspaces_added.subspaces,
        subspaces_removed: subspaces_removed.subspaces,
    })
}
```

### 4. Substreams.yaml Configuration

```yaml
modules:
  # Store modules for governance types and plugins
  - name: store_dao_governance_type
    kind: store
    updatePolicy: set
    valueType: string
    initialBlock: 515
    inputs:
      - map: map_governance_plugins_created
      - map: map_personal_admin_plugins_created

  - name: store_dao_plugins
    kind: store
    updatePolicy: set
    valueType: string
    initialBlock: 515
    inputs:
      - map: map_spaces_created
      - map: map_governance_plugins_created
      - map: map_personal_admin_plugins_created

  - name: store_plugin_to_dao
    kind: store
    updatePolicy: set
    valueType: string
    initialBlock: 515
    inputs:
      - map: map_spaces_created
      - map: map_governance_plugins_created
      - map: map_personal_admin_plugins_created

  # Map modules (simple event extraction, no validation)
  - name: map_spaces_created
    kind: map
    initialBlock: 515
    inputs:
      - source: sf.ethereum.type.v2.Block
    output:
      type: proto:schema.GeoSpacesCreated

  - name: map_members_added
    kind: map
    initialBlock: 515
    inputs:
      - source: sf.ethereum.type.v2.Block
    output:
      type: proto:schema.MembersAdded

  # ... other simple map handlers ...

  # Centralized validation in geo_out
  - name: geo_out
    kind: map
    initialBlock: 515
    inputs:
      - map: map_spaces_created
      - map: map_governance_plugins_created
      - map: map_votes_cast
      - map: map_members_added
      # ... all other map inputs ...
      - store: store_dao_governance_type
        mode: get
      - store: store_dao_plugins
        mode: get
      - store: store_plugin_to_dao
        mode: get
    output:
      type: proto:schema.GeoOutput
```

### 5. Implementation Steps

1. **Define Store modules in `substreams.yaml`**:
   - Configure simple store handlers without validation logic
   - Set up proper dependencies with initialBlock: 67162

2. **Implement simple store handler functions**:
   - Store DAO→Plugin and Plugin→DAO mappings
   - Store governance type for each DAO
   - No validation at storage time

3. **Keep map handlers simple**:
   - Focus only on event extraction from blockchain logs
   - No validation logic in individual handlers

4. **Implement centralized validation in `geo_out`**:
   - Single point of validation for all plugin-based events
   - Progressive validation logic that handles first-time registrations
   - Consistent logic applied to all event types
   - Clean separation of concerns

5. **Add validation helper function**:
   - Create `validate_plugin_for_dao` helper in `helpers.rs`
   - Implement progressive validation logic
   - Handle both established governance types and first-time registrations

### 6. Benefits

- **Single Point of Validation**: All event filtering happens in `geo_out` 
- **Progressive Validation**: Handles first-time plugin registrations correctly while enforcing governance type separation
- **Consistent Logic**: Same validation applies to all event types uniformly
- **Clean Architecture**: Map handlers focus on extraction, `geo_out` handles business logic
- **Better Performance**: Only validates events that make it to final output with O(1) store lookups
- **Maintainable**: Much easier to understand and modify validation logic in one place
- **Complete Coverage**: Every plugin-based event is validated consistently
- **Data Integrity**: Prevents processing events from unregistered or invalid plugins
- **Security**: Malicious plugins cannot generate events for DAOs they don't belong to

### 7. Timing and Architecture Assumptions

**Updated Understanding**: Plugin registration can occur across multiple blocks, which the implementation handles correctly through progressive validation.

1. **Multi-Block Plugin Registration**: Space plugins may be created separately from governance/personal plugins across different blocks
2. **Progressive Store Population**: Stores are populated incrementally as plugins are registered
3. **First-Time Validation**: Events from newly registered plugins are allowed before governance type is established
4. **Plugin Creation Events**: Always allowed and never validated (they establish legitimate plugins)

### 8. Key Principles

1. **Centralized Validation**: All validation logic lives in one place (`geo_out`)
2. **Simple Stores**: Store handlers just store data, no complex validation logic
3. **Clean Separation**: Event extraction vs. business logic validation are separate concerns
4. **Consistent Filtering**: Same validation rules apply to all plugin-based events
5. **Performance**: O(1) lookups for all validations, single validation pass

This architecture ensures data integrity while maintaining clean, maintainable code with a single point of truth for validation logic.

## References

- [Substreams Stores Documentation](https://docs.substreams.dev/concepts-and-fundamentals/modules#store-modules)
- [DAO Plugin Event Mapping Documentation](./dao-plugin-event-mapping.md)
- [Geo Browser Contracts README](https://github.com/geobrowser/geo-contracts/blob/main/README.md)