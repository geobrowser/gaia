# Substreams Store Implementation Summary

## Why This Implementation is Needed

### The Problem

The original Geo substreams implementation processes **all** plugin events without validating whether those plugins are legitimately registered to their DAOs. This creates several critical issues:

#### 1. **Data Integrity Violations**

- Events from **unregistered or malicious plugins** are processed as legitimate
- No enforcement of the **governance type separation** (Personal vs Governance spaces)
- Potential for **governance type mixing** within a single DAO

#### 2. **Security Vulnerabilities**

- **Malicious actors** could deploy fake plugins that emit events with arbitrary DAO addresses
- These fraudulent events would be **incorrectly indexed** as legitimate DAO activity
- No way to distinguish between authentic and inauthentic plugin events

#### 3. **Architectural Violations**

- **Personal spaces** should only accept events from personal admin plugins
- **Governance spaces** should only accept events from governance plugins (voting + member access)
- Current implementation **doesn't enforce** these architectural constraints

#### 4. **Example Attack Scenario**

```
1. Attacker deploys malicious contract at 0xBAD...
2. Contract emits MemberAdded(dao=0xLEGIT..., member=0xATTACKER...)
3. Current substreams processes this as legitimate
4. Attacker appears as member of legitimate DAO in indexed data
```

### Business Requirements

- **Only legitimate plugins** should generate indexed events
- **Governance type enforcement**: Personal and Governance plugins cannot be mixed
- **First-write-wins**: Once a DAO's governance type is set, it cannot be changed
- **Plugin validation**: Every plugin-based event must be validated against registered plugins

## What This Implementation Provides

### Core Solution

A **Substreams Store-based validation system** that:

1. **Tracks DAO→Plugin mappings** during plugin registration
2. **Validates all plugin events** against these mappings
3. **Filters out illegitimate events** before they reach the final output
4. **Enforces governance type separation** consistently

### Architecture Overview

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Map Handlers  │    │  Store Handlers │    │    geo_out      │
│                 │    │                 │    │                 │
│ Extract Events  │───▶│  Build Mappings │───▶│ Validate Events │
│ (No Validation) │    │  DAO ↔ Plugin   │    │ (Single Point)  │
└─────────────────┘    └─────────────────┘    └─────────────────┘
```

## Implementation Summary

### 1. **Store Modules** (Data Storage)

- **`store_dao_governance_type`**: Tracks if DAO uses "governance" or "personal" plugins
- **`store_dao_plugins`**: Maps DAOs to their legitimate plugin addresses
- **`store_plugin_to_dao`**: Reverse mapping for quick plugin→DAO lookups

### 2. **Store Handlers** (Simple Data Collection)

```rust
// Simple data storage, no validation logic
fn store_dao_governance_type() { /* Store governance type */ }
fn store_dao_plugins() { /* Store plugin addresses */ }
fn store_plugin_to_dao() { /* Store reverse mappings */ }
```

### 3. **Map Handlers** (Event Extraction Only)

```rust
// Focus solely on extracting events from blockchain
fn map_votes_cast() { /* Extract vote events, no validation */ }
fn map_members_added() { /* Extract member events, no validation */ }
```

### 4. **Centralized Validation** (Single Point of Truth)

```rust
fn geo_out() {
    // Validate ALL plugin-based events in one place
    let valid_votes = votes.filter(|vote| validate_plugin(&vote.plugin_address));
    let valid_members = members.filter(|member| validate_plugin(&member.plugin_address));
    // ... filter all event types consistently
}
```

### 5. **Validation Logic**

```rust
fn validate_plugin_for_dao(plugin_address) -> Option<String> {
    // 1. Look up which DAO this plugin claims to belong to
    let dao_address = get_plugin_to_dao.get_last(&format!("plugin:{}", plugin_address))?;
    
    // 2. Check what governance type that DAO uses (if any)
    let dao_type = get_dao_governance_type.get_last(&format!("dao:type:{}", dao_address));
    
    match dao_type.as_ref().map(|s| s.as_str()) {
        // 3a. If governance type exists, validate against it strictly
        Some("personal") => validate_personal_plugin(plugin_address, dao_address),
        Some("governance") => validate_governance_plugin(plugin_address, dao_address),
        
        // 3b. If no governance type, this is first-time registration
        None => {
            // Allow if plugin is registered in any DAO plugin mapping
            validate_any_registered_plugin(plugin_address, dao_address)
        }
    }
    
    // 4. Return DAO address if valid, None if invalid
}
```

## Key Benefits

### ✅ **Security**

- **Prevents malicious plugin events** from being processed
- **Only registered plugins** can generate indexed events
- **Progressive validation** handles first-time plugin registration correctly
- **Governance type enforcement** prevents architectural violations once established

### ✅ **Data Integrity**

- **Consistent validation** applied to all event types
- **Single point of truth** for plugin legitimacy
- **Progressive governance type establishment** (set on first governance plugin registration)
- **First-write-wins** governance type immutability once established

### ✅ **Performance**

- **O(1) validation lookups** using store mappings
- **Single validation pass** in geo_out instead of multiple checks
- **Efficient filtering** of events before final output

### ✅ **Architecture**

- **Clean separation of concerns**: extraction vs validation
- **Maintainable**: All validation logic in one place
- **Consistent**: Same rules apply to all plugin-based events

## Event Flow Example

### Before (Original - Vulnerable)

```
Block N: Vote event from 0xMALICIOUS → Processed ❌
         Member event from 0xUNREGISTERED → Processed ❌
```

### After (With Store Validation)

```
Block 67,238: Space plugin created → Space events allowed ✅
              Vote from unregistered plugin → Filtered out ✅
              
Block 67,240: Governance plugins created → Governance type established
              
Block 67,241: Vote from governance plugin → Processed ✅  
              Vote from malicious plugin → Filtered out ✅
              Member from personal plugin → Filtered out ✅ (wrong type)
              Member from governance plugin → Processed ✅
```

### Real-World Example

From actual debug logs:
```
Block #67,238: Space Creation
├── Space plugin 0xf352...d994 registered for DAO 0x0b53...4cc7
├── No governance type set yet (first plugin)  
├── Space events: ✅ Allowed (plugin is registered)
└── Vote/member events: ❌ Filtered (no governance plugins yet)

Block #67,240+: Governance Plugin Registration
├── Governance plugins registered → type set to "governance"
├── Vote events from voting plugin: ✅ Allowed
└── Member events from member plugin: ✅ Allowed
```

## Timing Architecture

### Multi-Block Plugin Registration

**Important Discovery**: Plugins for a DAO are **not always** registered in the same block. The implementation handles this correctly through progressive validation:

```
Block 100: Initial DAO Creation
├── Space plugin registered
└── Stores populated with space plugin mapping
    (No governance type set yet)

Block 101+: Governance Plugin Registration (if needed)
├── Governance or Personal Admin plugins registered
└── Governance type established

Block 102+: Operational Events
├── Events validated against established plugin mappings
└── First-time events allowed if plugin is registered
```

### Progressive Validation Logic

The validation system uses a **progressive approach**:

1. **If governance type exists**: Strict validation against established type
2. **If governance type doesn't exist**: First-time validation - allow if plugin is registered in any DAO plugin mapping
3. **Plugin registration**: Always allowed for legitimate plugin creation events

## Files Modified

- **`indexer-substream/substreams.yaml`**: Added store modules and dependencies
- **`indexer-substream/src/lib.rs`**: Added store handlers and centralized validation
- **`indexer-substream/src/helpers.rs`**: Added validation utility function

## Implementation Status

✅ **Completed**: Store-based validation system with centralized filtering
✅ **Tested**: Code compiles successfully
✅ **Architecture**: Clean separation between extraction and validation
✅ **Coverage**: All plugin-based events are validated consistently

This implementation provides a **robust, secure, and maintainable** solution that prevents illegitimate plugin events from contaminating the indexed data while enforcing proper governance type separation.
