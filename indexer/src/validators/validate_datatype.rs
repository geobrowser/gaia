/// Functions for validating values based on DataType.
use super::error::ValidationError;
use super::{
    validate_checkbox, validate_number_comprehensive, validate_point, validate_text,
    validate_time_comprehensive,
};
use crate::models::properties::DataType;
use chrono::{DateTime, Utc};

/// Represents a validated value that can be one of several types
#[derive(Debug, Clone, PartialEq)]
pub enum ValidatedValue {
    Text(String),
    Number(f64),
    Checkbox(bool),
    Time(DateTime<Utc>),
    Point(super::validate_point::Point),
}

/// Validates a string value according to the specified DataType.
///
/// # Arguments
///
/// * `data_type` - The DataType to validate against
/// * `value` - The string value to validate
///
/// # Returns
///
/// * `Ok(ValidatedValue)` - If the value is valid for the given DataType
/// * `Err(ValidationError)` - If the value is invalid
pub fn validate_by_datatype(
    data_type: DataType,
    value: &str,
) -> Result<ValidatedValue, ValidationError> {
    match data_type {
        DataType::String => {
            let validated = validate_text(value)?;
            Ok(ValidatedValue::Text(validated))
        }
        DataType::Number => {
            let validated = validate_number_comprehensive(value)?;
            Ok(ValidatedValue::Number(validated))
        }
        DataType::Boolean => {
            let validated = validate_checkbox(value)?;
            Ok(ValidatedValue::Checkbox(validated))
        }
        DataType::Time => {
            let validated = validate_time_comprehensive(value)?;
            Ok(ValidatedValue::Time(validated))
        }
        DataType::Point => {
            let validated = validate_point(value)?;
            Ok(ValidatedValue::Point(validated))
        }
        DataType::Relation => {
            // Relations are not validated at the value level
            // Return the original string as text
            Ok(ValidatedValue::Text(value.to_string()))
        }
    }
}

/// Validates a string value according to the specified DataType and returns the original string if valid.
///
/// This is useful when you want to validate but keep the original string format.
///
/// # Arguments
///
/// * `data_type` - The DataType to validate against
/// * `value` - The string value to validate
///
/// # Returns
///
/// * `Ok(String)` - If the value is valid for the given DataType (returns original string)
/// * `Err(ValidationError)` - If the value is invalid
pub fn validate_string_by_datatype(
    data_type: DataType,
    value: &str,
) -> Result<String, ValidationError> {
    // Validate using the typed validator but return the original string
    validate_by_datatype(data_type, value)?;
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::properties::DataType;

    #[test]
    fn test_validate_by_datatype_text() {
        let result = validate_by_datatype(DataType::String, "Hello World");
        assert!(matches!(result, Ok(ValidatedValue::Text(_))));

        if let Ok(ValidatedValue::Text(text)) = result {
            assert_eq!(text, "Hello World");
        }
    }

    #[test]
    fn test_validate_by_datatype_number() {
        let result = validate_by_datatype(DataType::Number, "123.45");
        assert!(matches!(result, Ok(ValidatedValue::Number(_))));

        if let Ok(ValidatedValue::Number(num)) = result {
            assert_eq!(num, 123.45);
        }

        // Invalid number
        assert!(validate_by_datatype(DataType::Number, "abc").is_err());
    }

    #[test]
    fn test_validate_by_datatype_checkbox() {
        let result = validate_by_datatype(DataType::Boolean, "1");
        assert!(matches!(result, Ok(ValidatedValue::Checkbox(_))));

        if let Ok(ValidatedValue::Checkbox(checked)) = result {
            assert_eq!(checked, true);
        }

        let result = validate_by_datatype(DataType::Boolean, "0");
        if let Ok(ValidatedValue::Checkbox(checked)) = result {
            assert_eq!(checked, false);
        }

        // Invalid checkbox
        assert!(validate_by_datatype(DataType::Boolean, "2").is_err());
    }

    #[test]
    fn test_validate_by_datatype_time() {
        let result = validate_by_datatype(DataType::Time, "2023-12-25T10:30:00Z");
        assert!(matches!(result, Ok(ValidatedValue::Time(_))));

        // Invalid time
        assert!(validate_by_datatype(DataType::Time, "invalid-time").is_err());
    }

    #[test]
    fn test_validate_by_datatype_point() {
        let result = validate_by_datatype(DataType::Point, "1.5,2.5");
        assert!(matches!(result, Ok(ValidatedValue::Point(_))));

        if let Ok(ValidatedValue::Point(point)) = result {
            assert_eq!(point.x, 1.5);
            assert_eq!(point.y, 2.5);
        }

        // Invalid point
        assert!(validate_by_datatype(DataType::Point, "invalid-point").is_err());
    }

    #[test]
    fn test_validate_by_datatype_relation() {
        let result = validate_by_datatype(DataType::Relation, "some-relation-id");
        assert!(matches!(result, Ok(ValidatedValue::Text(_))));

        if let Ok(ValidatedValue::Text(text)) = result {
            assert_eq!(text, "some-relation-id");
        }
    }

    #[test]
    fn test_validate_string_by_datatype() {
        // Valid cases
        assert_eq!(
            validate_string_by_datatype(DataType::String, "Hello"),
            Ok("Hello".to_string())
        );
        assert_eq!(
            validate_string_by_datatype(DataType::Number, "123.45"),
            Ok("123.45".to_string())
        );
        assert_eq!(
            validate_string_by_datatype(DataType::Boolean, "1"),
            Ok("1".to_string())
        );
        assert_eq!(
            validate_string_by_datatype(DataType::Time, "2023-12-25T10:30:00Z"),
            Ok("2023-12-25T10:30:00Z".to_string())
        );
        assert_eq!(
            validate_string_by_datatype(DataType::Point, "1.5,2.5"),
            Ok("1.5,2.5".to_string())
        );
        assert_eq!(
            validate_string_by_datatype(DataType::Relation, "relation-id"),
            Ok("relation-id".to_string())
        );

        // Invalid cases
        assert!(validate_string_by_datatype(DataType::Number, "abc").is_err());
        assert!(validate_string_by_datatype(DataType::Boolean, "2").is_err());
        assert!(validate_string_by_datatype(DataType::Time, "invalid-time").is_err());
        assert!(validate_string_by_datatype(DataType::Point, "invalid-point").is_err());
    }
}
