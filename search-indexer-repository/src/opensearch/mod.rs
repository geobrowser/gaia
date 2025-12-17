//! OpenSearch implementation of the search index provider.
//!
//! This module provides a concrete implementation of `SearchIndexProvider`
//! using OpenSearch as the backend.

mod bulk;
mod index_config;
mod provider;
mod unset_document_properties;

pub use bulk::{
    execute_bulk, parse_bulk_response, BulkAction, BulkOperationMeta, BulkScript, BulkScriptBody,
    BulkUpdateBody,
};
pub use index_config::{get_index_settings, get_versioned_index_name, IndexConfig, INDEX_NAME};
pub use provider::OpenSearchProvider;
pub use unset_document_properties::{create_unset_properties_script, validate_property_keys};
