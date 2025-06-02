/// Functions for validating number strings.

use super::error::ValidationError;

/// Validates if the input string represents a valid number (integer or float).
///
/// # Arguments
///
/// * `input` - A string slice that contains the number to validate
///
/// # Returns
///
/// * `Ok(f64)` - If the input is a valid number
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_number(input: &str) -> Result<f64, ValidationError> {
    input.parse::<f64>().map_err(|_| ValidationError::ParseFailure)
}

/// A more comprehensive number validator that checks for specific patterns.
///
/// # Arguments
///
/// * `input` - A string slice that contains the number to validate
///
/// # Returns
///
/// * `Ok(f64)` - If the input is a valid number
/// * `Err(ValidationError)` - If the input is invalid, with a specific error type
pub fn validate_number_comprehensive(input: &str) -> Result<f64, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }

    // Check if it has valid number characters
    let valid_chars = |c: char| {
        c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'
    };
    if !input.chars().all(valid_chars) {
        return Err(ValidationError::InvalidCharacters);
    }

    // Count decimal points (should be 0 or 1)
    let decimal_points = input.chars().filter(|&c| c == '.').count();
    if decimal_points > 1 {
        return Err(ValidationError::MultipleDecimalPoints);
    }

    // Finally try to parse
    input.parse::<f64>().map_err(|_| ValidationError::ParseFailure)
}

/// Validates if the input string represents a valid integer.
///
/// # Arguments
///
/// * `input` - A string slice that contains the integer to validate
///
/// # Returns
///
/// * `Ok(i64)` - If the input is a valid integer
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_integer(input: &str) -> Result<i64, ValidationError> {
    input.parse::<i64>().map_err(|_| ValidationError::ParseFailure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_number() {
        // Valid cases
        assert!(validate_number("123").is_ok());
        assert!(validate_number("123.45").is_ok());
        assert!(validate_number("0").is_ok());
        assert!(validate_number("-42").is_ok());
        assert!(validate_number("-42.75").is_ok());
        assert!(validate_number("1e10").is_ok());
        assert!(validate_number("1.5e-3").is_ok());
        
        // Invalid cases
        assert_eq!(validate_number("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_number("abc").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_number("12.34.56").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_number_comprehensive() {
        // Valid cases
        assert!(validate_number_comprehensive("123").is_ok());
        assert!(validate_number_comprehensive("123.45").is_ok());
        assert!(validate_number_comprehensive("0").is_ok());
        assert!(validate_number_comprehensive("-42").is_ok());
        assert!(validate_number_comprehensive("-42.75").is_ok());
        assert!(validate_number_comprehensive("1e10").is_ok());
        assert!(validate_number_comprehensive("1.5e-3").is_ok());
        
        // Invalid cases
        assert_eq!(validate_number_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_number_comprehensive("abc").err(), Some(ValidationError::InvalidCharacters));
        assert_eq!(validate_number_comprehensive("12.34.56").err(), Some(ValidationError::MultipleDecimalPoints));
        assert_eq!(validate_number_comprehensive("1,234.56").err(), Some(ValidationError::InvalidCharacters));
        assert_eq!(validate_number_comprehensive("12$34").err(), Some(ValidationError::InvalidCharacters));
    }

    #[test]
    fn test_validate_integer() {
        // Valid cases
        assert!(validate_integer("123").is_ok());
        assert!(validate_integer("0").is_ok());
        assert!(validate_integer("-42").is_ok());
        
        // Invalid cases
        assert_eq!(validate_integer("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_integer("abc").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_integer("12.34").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_integer("1e10").err(), Some(ValidationError::ParseFailure));
    }
}