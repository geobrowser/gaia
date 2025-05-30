/// Functions for validating decimal numbers.

use super::error::ValidationError;

/// Validates if the input string represents a number with exactly two decimal places.
///
/// # Arguments
///
/// * `input` - A string slice that contains the number to validate
///
/// # Returns
///
/// * `Ok(f64)` - If the input is a valid number with exactly two decimal places
/// * `Err(ValidationError)` - If the input is invalid, with a specific error type
pub fn validate_two_decimal_places(input: &str) -> Result<f64, ValidationError> {
    let number: f64 = input.parse().map_err(|_| ValidationError::ParseFailure)?;

    if let Some(pos) = input.find('.') {
        let decimal_places = input.len() - pos - 1;
        if decimal_places == 2 {
            Ok(number)
        } else {
            Err(ValidationError::IncorrectDecimalPlaces(2, decimal_places))
        }
    } else {
        Err(ValidationError::MissingDecimalPoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_two_decimal_places() {
        // Valid cases
        assert!(validate_two_decimal_places("12.34").is_ok());
        assert!(validate_two_decimal_places("0.99").is_ok());
        assert!(validate_two_decimal_places("100.00").is_ok());
        
        // Invalid cases
        assert_eq!(validate_two_decimal_places("5.6").err(), 
                  Some(ValidationError::IncorrectDecimalPlaces(2, 1)));
        assert_eq!(validate_two_decimal_places("7").err(), 
                  Some(ValidationError::MissingDecimalPoint));
        assert_eq!(validate_two_decimal_places("12.345").err(), 
                  Some(ValidationError::IncorrectDecimalPlaces(2, 3)));
        assert_eq!(validate_two_decimal_places("abc").err(), 
                  Some(ValidationError::ParseFailure));
    }
}