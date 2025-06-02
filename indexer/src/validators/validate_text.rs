/// Functions for validating text strings.

use super::error::ValidationError;

/// Validates if the input string is valid text.
/// 
/// Allows empty strings and most characters, but rejects control characters
/// (except for common whitespace like spaces, tabs, and newlines).
///
/// # Arguments
///
/// * `input` - A string slice that contains the text to validate
///
/// # Returns
///
/// * `Ok(String)` - If the input is valid text
/// * `Err(ValidationError)` - If the input contains invalid characters
pub fn validate_text(input: &str) -> Result<String, ValidationError> {
    // Check for invalid control characters (except common whitespace)
    let has_invalid_chars = input.chars().any(|c| {
        c.is_control() && c != '\n' && c != '\r' && c != '\t'
    });
    
    if has_invalid_chars {
        return Err(ValidationError::InvalidCharacters);
    }
    
    Ok(input.to_string())
}

/// A more comprehensive text validator that also rejects empty input.
///
/// # Arguments
///
/// * `input` - A string slice that contains the text to validate
///
/// # Returns
///
/// * `Ok(String)` - If the input is valid non-empty text
/// * `Err(ValidationError)` - If the input is empty or contains invalid characters
pub fn validate_text_comprehensive(input: &str) -> Result<String, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }
    
    // Use the basic validator for character validation
    validate_text(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_text() {
        // Valid cases
        assert!(validate_text("Hello World").is_ok());
        assert!(validate_text("").is_ok()); // Empty string is allowed
        assert!(validate_text("123").is_ok());
        assert!(validate_text("Hello\nWorld").is_ok()); // Newlines allowed
        assert!(validate_text("Hello\tWorld").is_ok()); // Tabs allowed
        assert!(validate_text("Special chars: !@#$%^&*()").is_ok());
        
        // Invalid cases
        assert_eq!(validate_text("Hello\x00World").err(), Some(ValidationError::InvalidCharacters)); // Null byte
        assert_eq!(validate_text("Hello\x07World").err(), Some(ValidationError::InvalidCharacters)); // Bell character
    }

    #[test]
    fn test_validate_text_comprehensive() {
        // Valid cases
        assert!(validate_text_comprehensive("Hello World").is_ok());
        assert!(validate_text_comprehensive("123").is_ok());
        assert!(validate_text_comprehensive("Hello\nWorld").is_ok());
        
        // Invalid cases
        assert_eq!(validate_text_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_text_comprehensive("Hello\x00World").err(), Some(ValidationError::InvalidCharacters));
    }
}