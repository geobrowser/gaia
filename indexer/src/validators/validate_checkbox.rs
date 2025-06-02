/// Functions for validating checkbox (boolean) strings.

use super::error::ValidationError;

/// Validates if the input string represents a valid checkbox value.
/// 
/// Valid checkbox values are "0" (false) and "1" (true).
///
/// # Arguments
///
/// * `input` - A string slice that contains the checkbox value to validate
///
/// # Returns
///
/// * `Ok(bool)` - If the input is a valid checkbox value ("0" -> false, "1" -> true)
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_checkbox(input: &str) -> Result<bool, ValidationError> {
    match input {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(ValidationError::ParseFailure),
    }
}

/// A more comprehensive checkbox validator that provides specific error messages.
///
/// # Arguments
///
/// * `input` - A string slice that contains the checkbox value to validate
///
/// # Returns
///
/// * `Ok(bool)` - If the input is a valid checkbox value
/// * `Err(ValidationError)` - If the input is invalid, with a specific error type
pub fn validate_checkbox_comprehensive(input: &str) -> Result<bool, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }
    
    // Use the basic validator
    validate_checkbox(input)
}

/// Validates if the input string represents a valid checkbox value and returns the string representation.
/// 
/// This is useful when you want to validate but keep the original string format.
///
/// # Arguments
///
/// * `input` - A string slice that contains the checkbox value to validate
///
/// # Returns
///
/// * `Ok(String)` - If the input is a valid checkbox value (returns the original string)
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_checkbox_string(input: &str) -> Result<String, ValidationError> {
    validate_checkbox(input)?;
    Ok(input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_checkbox() {
        // Valid cases
        assert_eq!(validate_checkbox("0"), Ok(false));
        assert_eq!(validate_checkbox("1"), Ok(true));
        
        // Invalid cases
        assert_eq!(validate_checkbox("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("2").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("true").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("false").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("yes").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("no").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("01").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox("10").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_checkbox_comprehensive() {
        // Valid cases
        assert_eq!(validate_checkbox_comprehensive("0"), Ok(false));
        assert_eq!(validate_checkbox_comprehensive("1"), Ok(true));
        
        // Invalid cases
        assert_eq!(validate_checkbox_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_checkbox_comprehensive("2").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox_comprehensive("true").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox_comprehensive("false").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_checkbox_string() {
        // Valid cases
        assert_eq!(validate_checkbox_string("0"), Ok("0".to_string()));
        assert_eq!(validate_checkbox_string("1"), Ok("1".to_string()));
        
        // Invalid cases
        assert_eq!(validate_checkbox_string("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox_string("2").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_checkbox_string("true").err(), Some(ValidationError::ParseFailure));
    }
}