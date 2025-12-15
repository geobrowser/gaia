//! Utility functions for the search indexer repository.

use uuid::Uuid;

use crate::errors::SearchIndexError;

/// Parse and validate entity_id and space_id from string UUIDs.
///
/// This utility function can be used across different implementations to parse
/// and validate UUID strings for entity and space identifiers.
///
/// # Arguments
///
/// * `entity_id_str` - The entity ID as a string
/// * `space_id_str` - The space ID as a string
///
/// # Returns
///
/// * `Ok((Uuid, Uuid))` - Parsed entity_id and space_id
/// * `Err(SearchIndexError)` - If either UUID is invalid
///
/// # Example
///
/// ```
/// use search_indexer_repository::parse_entity_and_space_ids;
///
/// let (entity_id, space_id) = parse_entity_and_space_ids(
///     "550e8400-e29b-41d4-a716-446655440000",
///     "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
/// ).expect("valid UUIDs");
/// ```
pub fn parse_entity_and_space_ids(
    entity_id_str: &str,
    space_id_str: &str,
) -> Result<(Uuid, Uuid), SearchIndexError> {
    let entity_id = Uuid::parse_str(entity_id_str)
        .map_err(|e| SearchIndexError::validation(format!("Invalid entity_id: {}", e)))?;
    let space_id = Uuid::parse_str(space_id_str)
        .map_err(|e| SearchIndexError::validation(format!("Invalid space_id: {}", e)))?;
    Ok((entity_id, space_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_entity_and_space_ids() {
        let entity_id_str = "550e8400-e29b-41d4-a716-446655440000";
        let space_id_str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";

        let result = parse_entity_and_space_ids(entity_id_str, space_id_str);
        assert!(result.is_ok());

        let (entity_id, space_id) = result.unwrap();
        assert_eq!(entity_id.to_string(), entity_id_str);
        assert_eq!(space_id.to_string(), space_id_str);
    }

    #[test]
    fn test_parse_entity_and_space_ids_invalid_entity() {
        let result = parse_entity_and_space_ids("invalid", "6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_parse_entity_and_space_ids_invalid_space() {
        let result = parse_entity_and_space_ids("550e8400-e29b-41d4-a716-446655440000", "invalid");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }
}
