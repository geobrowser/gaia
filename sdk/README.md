# SDK

A Rust SDK for interacting with Fe knowledge graph.

## Overview

Entities in The Graph have attributes that provide semantic meaning. Each type of attribute has a unique identifier. This SDK provides constants for well-known attribute IDs, such as name and description attributes.

## Usage

### Entity Attribute IDs

```rust
use sdk::core::ids::{NAME_PROPERTY_ID, DESCRIPTION_PROPERTY_ID};

// Use these constants when creating or querying entity attributes
let name_property_id = NAME_PROPERTY_ID;
let description_property_id = DESCRIPTION_PROPERTY_ID;
```

