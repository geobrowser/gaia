//! Utilities for unsetting document properties in OpenSearch.
//!
//! This module provides functions for validating property keys and creating
//! Painless scripts to safely remove fields from documents.

use crate::errors::SearchIndexError;

/// Validate property keys contain only ASCII alphanumeric characters and underscores.
pub fn validate_property_keys(property_keys: &[String]) -> Result<(), SearchIndexError> {
    if property_keys.is_empty() {
        return Err(SearchIndexError::validation(
            "At least one property key must be provided".to_string(),
        ));
    }

    for key in property_keys {
        if key.is_empty() {
            return Err(SearchIndexError::validation(
                "Property keys cannot be empty".to_string(),
            ));
        }

        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(SearchIndexError::validation(format!(
                "Property key '{}' contains invalid characters. Only ASCII alphanumeric characters and underscores are allowed",
                key
            )));
        }
    }

    Ok(())
}

/// Create a OpenSearch Painless script to safely remove fields from a document.
pub fn create_unset_properties_script(
    property_keys: &[String],
) -> Result<String, SearchIndexError> {
    // Validate property keys before generating script
    validate_property_keys(property_keys)?;

    Ok(property_keys
        .iter()
        .map(|key| {
            // Escape the key for use in Painless script
            // Since we've validated the key contains only ASCII alphanumeric and underscore,
            // we don't need complex escaping, but we'll still quote it properly
            format!(
                "if (ctx._source.containsKey(\"{}\")) {{ ctx._source.remove(\"{}\") }}",
                key, key
            )
        })
        .collect::<Vec<_>>()
        .join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_property_keys_valid() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "entity_global_score".to_string(),
            "test123".to_string(),
            "a".to_string(),
            "A".to_string(),
            "a1".to_string(),
            "_private".to_string(),
        ];
        assert!(validate_property_keys(&keys).is_ok());
    }

    #[test]
    fn test_validate_property_keys_empty_vec() {
        let keys = vec![];
        let result = validate_property_keys(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_validate_property_keys_empty_string() {
        let keys = vec!["".to_string()];
        let result = validate_property_keys(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_validate_property_keys_invalid_characters() {
        let test_cases = vec![
            ("name-with-dash", "contains dash"),
            ("name.with.dot", "contains dot"),
            ("name with space", "contains space"),
            ("name@symbol", "contains @"),
            ("name#hash", "contains #"),
            ("name$dollar", "contains $"),
            ("name%percent", "contains %"),
            ("name&and", "contains &"),
            ("name*star", "contains *"),
            ("name+plus", "contains +"),
            ("name=equals", "contains ="),
            ("name[ bracket", "contains ["),
            ("name] bracket", "contains ]"),
            ("name{ brace", "contains {"),
            ("name} brace", "contains }"),
            ("name|pipe", "contains |"),
            ("name\\backslash", "contains backslash"),
            ("name/forward", "contains forward slash"),
            ("name?question", "contains ?"),
            ("name:colon", "contains :"),
            ("name;semicolon", "contains ;"),
            ("name\"quote", "contains quote"),
            ("name'apostrophe", "contains apostrophe"),
            ("name<less", "contains <"),
            ("name>greater", "contains >"),
            ("name,comma", "contains comma"),
        ];

        for (key, description) in test_cases {
            let keys = vec![key.to_string()];
            let result = validate_property_keys(&keys);
            assert!(
                result.is_err(),
                "Expected error for key '{}' ({})",
                key,
                description
            );
            assert!(
                matches!(result.unwrap_err(), SearchIndexError::ValidationError(_)),
                "Expected ValidationError for key '{}'",
                key
            );
        }
    }

    #[test]
    fn test_validate_property_keys_mixed_valid_invalid() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "invalid-key".to_string(), // Invalid
        ];
        let result = validate_property_keys(&keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_unset_properties_script_single_key() {
        let keys = vec!["name".to_string()];
        let script = create_unset_properties_script(&keys).unwrap();
        assert_eq!(
            script,
            "if (ctx._source.containsKey(\"name\")) { ctx._source.remove(\"name\") }"
        );
    }

    #[test]
    fn test_create_unset_properties_script_multiple_keys() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "avatar".to_string(),
        ];
        let script = create_unset_properties_script(&keys).unwrap();
        assert!(script.contains("name"));
        assert!(script.contains("description"));
        assert!(script.contains("avatar"));
        assert!(script.contains("containsKey"));
        assert!(script.contains("remove"));
        // Should have semicolons separating the statements
        assert_eq!(script.matches(';').count(), 2);
    }

    #[test]
    fn test_create_unset_properties_script_multiple_keys_exact_format() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "avatar".to_string(),
            "cover".to_string(),
            "entity_global_score".to_string(),
        ];
        let script = create_unset_properties_script(&keys).unwrap();

        // Verify exact script format
        let expected_script = "if (ctx._source.containsKey(\"name\")) { ctx._source.remove(\"name\") }; if (ctx._source.containsKey(\"description\")) { ctx._source.remove(\"description\") }; if (ctx._source.containsKey(\"avatar\")) { ctx._source.remove(\"avatar\") }; if (ctx._source.containsKey(\"cover\")) { ctx._source.remove(\"cover\") }; if (ctx._source.containsKey(\"entity_global_score\")) { ctx._source.remove(\"entity_global_score\") }";
        assert_eq!(script, expected_script);
    }

    #[test]
    fn test_create_unset_properties_script_with_underscore() {
        let keys = vec!["entity_global_score".to_string()];
        let script = create_unset_properties_script(&keys).unwrap();
        assert_eq!(
            script,
            "if (ctx._source.containsKey(\"entity_global_score\")) { ctx._source.remove(\"entity_global_score\") }"
        );
    }

    #[test]
    fn test_create_unset_properties_script_empty() {
        let keys = vec![];
        let result = create_unset_properties_script(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_create_unset_properties_script_invalid_key() {
        let keys = vec!["invalid-key".to_string()];
        let result = create_unset_properties_script(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }
}

