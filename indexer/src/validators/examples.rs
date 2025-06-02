//! Usage examples for DataType validators.
//! 
//! This module provides examples of how to use the various validators
//! for different DataTypes in the system.

#[cfg(test)]
mod examples {
    use crate::models::properties::DataType;
    use crate::validators::{
        validate_by_datatype, validate_string_by_datatype, ValidatedValue,
        validate_text, validate_number_comprehensive, validate_checkbox,
        validate_time_comprehensive, validate_point, Point,
        ValidationError
    };

    #[test]
    fn example_validate_text_data() {
        // Text validation examples
        let text_values = vec![
            "Hello World",
            "Multi-line\ntext\nwith\nbreaks",
            "Special chars: !@#$%^&*()",
            "Numbers as text: 12345",
            "", // Empty string is valid for text
        ];

        for value in text_values {
            match validate_by_datatype(DataType::Text, value) {
                Ok(ValidatedValue::Text(text)) => {
                    println!("✓ Valid text: '{}'", text);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => println!("✗ Invalid text '{}': {}", value, e),
            }
        }
    }

    #[test]
    fn example_validate_number_data() {
        // Number validation examples
        let number_values = vec![
            ("123", true),
            ("123.45", true),
            ("-42.75", true),
            ("0", true),
            ("1e10", true),
            ("1.5e-3", true),
            ("abc", false),
            ("12.34.56", false),
            ("", false),
        ];

        for (value, should_be_valid) in number_values {
            match validate_by_datatype(DataType::Number, value) {
                Ok(ValidatedValue::Number(num)) => {
                    assert!(should_be_valid, "Expected '{}' to be invalid", value);
                    println!("✓ Valid number: {} -> {}", value, num);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => {
                    assert!(!should_be_valid, "Expected '{}' to be valid", value);
                    println!("✗ Invalid number '{}': {}", value, e);
                }
            }
        }
    }

    #[test]
    fn example_validate_checkbox_data() {
        // Checkbox validation examples
        let checkbox_values = vec![
            ("0", Some(false)),
            ("1", Some(true)),
            ("true", None), // Invalid
            ("false", None), // Invalid
            ("2", None), // Invalid
            ("", None), // Invalid
        ];

        for (value, expected) in checkbox_values {
            match validate_by_datatype(DataType::Checkbox, value) {
                Ok(ValidatedValue::Checkbox(checked)) => {
                    assert_eq!(Some(checked), expected, "Unexpected checkbox value for '{}'", value);
                    println!("✓ Valid checkbox: '{}' -> {}", value, checked);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => {
                    assert!(expected.is_none(), "Expected '{}' to be valid", value);
                    println!("✗ Invalid checkbox '{}': {}", value, e);
                }
            }
        }
    }

    #[test]
    fn example_validate_time_data() {
        // Time validation examples
        let time_values = vec![
            ("2023-12-25T10:30:00Z", true),
            ("2023-12-25T10:30:00+00:00", true),
            ("2023-12-25 10:30:00", true),
            ("2023-12-25", true),
            ("1703505000", true), // Unix timestamp
            ("invalid-date", false),
            ("2023-13-25", false), // Invalid month
            ("", false),
        ];

        for (value, should_be_valid) in time_values {
            match validate_by_datatype(DataType::Time, value) {
                Ok(ValidatedValue::Time(datetime)) => {
                    assert!(should_be_valid, "Expected '{}' to be invalid", value);
                    println!("✓ Valid time: '{}' -> {}", value, datetime);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => {
                    assert!(!should_be_valid, "Expected '{}' to be valid", value);
                    println!("✗ Invalid time '{}': {}", value, e);
                }
            }
        }
    }

    #[test]
    fn example_validate_point_data() {
        // Point validation examples
        let point_values = vec![
            ("1.5,2.5", Some(Point { x: 1.5, y: 2.5 })),
            ("(1.5,2.5)", Some(Point { x: 1.5, y: 2.5 })),
            ("1.5 2.5", Some(Point { x: 1.5, y: 2.5 })),
            (r#"{"x":1.5,"y":2.5}"#, Some(Point { x: 1.5, y: 2.5 })),
            ("0,0", Some(Point { x: 0.0, y: 0.0 })),
            ("-1.5,2.5", Some(Point { x: -1.5, y: 2.5 })),
            ("invalid-point", None),
            ("1.5", None), // Missing y coordinate
            ("", None),
        ];

        for (value, expected) in point_values {
            match validate_by_datatype(DataType::Point, value) {
                Ok(ValidatedValue::Point(point)) => {
                    assert_eq!(Some(point.clone()), expected, "Unexpected point value for '{}'", value);
                    println!("✓ Valid point: '{}' -> ({}, {})", value, point.x, point.y);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => {
                    assert!(expected.is_none(), "Expected '{}' to be valid", value);
                    println!("✗ Invalid point '{}': {}", value, e);
                }
            }
        }
    }

    #[test]
    fn example_validate_relation_data() {
        // Relation validation examples (relations just pass through as text)
        let relation_values = vec![
            "user-id-12345",
            "relation-abc-def",
            "some-uuid-string",
            "", // Even empty strings are valid for relations
        ];

        for value in relation_values {
            match validate_by_datatype(DataType::Relation, value) {
                Ok(ValidatedValue::Text(text)) => {
                    println!("✓ Valid relation: '{}'", text);
                    assert_eq!(text, value);
                }
                Ok(_) => panic!("Unexpected validated value type"),
                Err(e) => panic!("Relation validation should not fail: {}", e),
            }
        }
    }

    #[test]
    fn example_bulk_validation() {
        // Example of validating multiple values with different types
        let test_data = vec![
            (DataType::Text, "Hello World"),
            (DataType::Number, "123.45"),
            (DataType::Checkbox, "1"),
            (DataType::Time, "2023-12-25T10:30:00Z"),
            (DataType::Point, "1.5,2.5"),
            (DataType::Relation, "relation-id-123"),
        ];

        for (data_type, value) in test_data {
            match validate_string_by_datatype(data_type, value) {
                Ok(validated_string) => {
                    println!("✓ {:?}: '{}' -> '{}'", data_type, value, validated_string);
                    assert_eq!(validated_string, value); // Should return original string
                }
                Err(e) => {
                    panic!("Validation failed for {:?} with value '{}': {}", data_type, value, e);
                }
            }
        }
    }

    #[test]
    fn example_error_handling() {
        // Example of handling validation errors
        let invalid_cases = vec![
            (DataType::Number, "not-a-number"),
            (DataType::Checkbox, "maybe"),
            (DataType::Time, "not-a-time"),
            (DataType::Point, "not-a-point"),
        ];

        for (data_type, value) in invalid_cases {
            match validate_by_datatype(data_type, value) {
                Ok(_) => panic!("Expected validation to fail for {:?} with '{}'", data_type, value),
                Err(ValidationError::ParseFailure) => {
                    println!("✓ Correctly caught parse failure for {:?}: '{}'", data_type, value);
                }
                Err(ValidationError::EmptyInput) => {
                    println!("✓ Correctly caught empty input for {:?}: '{}'", data_type, value);
                }
                Err(ValidationError::InvalidCharacters) => {
                    println!("✓ Correctly caught invalid characters for {:?}: '{}'", data_type, value);
                }
                Err(e) => {
                    println!("✓ Caught validation error for {:?}: '{}' -> {}", data_type, value, e);
                }
            }
        }
    }
}