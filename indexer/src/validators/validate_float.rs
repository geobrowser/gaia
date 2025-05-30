/// Functions for validating floating point numbers.

use super::error::ValidationError;

/// Validates if the input string can be parsed as a float (f64)
pub fn validate_float(input: &str) -> Result<f64, ValidationError> {
    input.parse::<f64>().map_err(|_| ValidationError::ParseFailure)
}

/// A more comprehensive float validator that checks for specific patterns
pub fn validate_float_comprehensive(input: &str) -> Result<f64, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }

    // Check if it has valid float characters
    let valid_chars = |c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E';
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_float() {
        assert!(validate_float("123.45").is_ok());
        assert!(validate_float("0.0").is_ok());
        assert!(validate_float("-42.75").is_ok());
        assert!(validate_float("1e10").is_ok());
        assert_eq!(validate_float("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_float("abc").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_float_comprehensive() {
        assert!(validate_float_comprehensive("123.45").is_ok());
        assert!(validate_float_comprehensive("0.0").is_ok());
        assert!(validate_float_comprehensive("-42.75").is_ok());
        assert!(validate_float_comprehensive("1e10").is_ok());
        assert_eq!(validate_float_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_float_comprehensive("abc").err(), Some(ValidationError::InvalidCharacters));
        assert_eq!(validate_float_comprehensive("123..456").err(), Some(ValidationError::MultipleDecimalPoints));
        assert_eq!(validate_float_comprehensive("123.456.789").err(), Some(ValidationError::MultipleDecimalPoints));
        assert!(validate_float_comprehensive("1.2e-3").is_ok());
        assert_eq!(validate_float_comprehensive("1,234.56").err(), Some(ValidationError::InvalidCharacters));
    }
}