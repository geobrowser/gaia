/// Functions for validating time strings.

use super::error::ValidationError;
use chrono::{DateTime, NaiveDateTime, Utc};

/// Validates if the input string represents a valid time in ISO 8601/RFC 3339 format.
///
/// # Arguments
///
/// * `input` - A string slice that contains the time to validate
///
/// # Returns
///
/// * `Ok(DateTime<Utc>)` - If the input is a valid time string
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_time(input: &str) -> Result<DateTime<Utc>, ValidationError> {
    // Try to parse as RFC 3339 format first
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }
    
    // Try to parse as a naive datetime and assume UTC
    if let Ok(ndt) = NaiveDateTime::parse_from_str(input, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(ndt, Utc));
    }
    
    // Try to parse as just a date (assume midnight UTC)
    if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        if let Some(ndt) = date.and_hms_opt(0, 0, 0) {
            return Ok(DateTime::from_naive_utc_and_offset(ndt, Utc));
        }
    }
    
    Err(ValidationError::ParseFailure)
}

/// Validates if the input string represents a valid Unix timestamp.
///
/// # Arguments
///
/// * `input` - A string slice that contains the Unix timestamp to validate
///
/// # Returns
///
/// * `Ok(DateTime<Utc>)` - If the input is a valid Unix timestamp
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_unix_timestamp(input: &str) -> Result<DateTime<Utc>, ValidationError> {
    let timestamp: i64 = input.parse().map_err(|_| ValidationError::ParseFailure)?;
    
    DateTime::from_timestamp(timestamp, 0)
        .ok_or(ValidationError::ParseFailure)
}

/// A more comprehensive time validator that checks for various time formats.
///
/// # Arguments
///
/// * `input` - A string slice that contains the time to validate
///
/// # Returns
///
/// * `Ok(DateTime<Utc>)` - If the input is a valid time string
/// * `Err(ValidationError)` - If the input is invalid, with a specific error type
pub fn validate_time_comprehensive(input: &str) -> Result<DateTime<Utc>, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }
    
    // Try the basic time validator first
    if let Ok(dt) = validate_time(input) {
        return Ok(dt);
    }
    
    // Try Unix timestamp if the basic validator fails
    if let Ok(dt) = validate_unix_timestamp(input) {
        return Ok(dt);
    }
    
    Err(ValidationError::ParseFailure)
}

/// Validates if the input string represents a valid time and returns the string representation.
/// 
/// This is useful when you want to validate but keep the original string format.
///
/// # Arguments
///
/// * `input` - A string slice that contains the time to validate
///
/// # Returns
///
/// * `Ok(String)` - If the input is a valid time (returns the original string)
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_time_string(input: &str) -> Result<String, ValidationError> {
    validate_time_comprehensive(input)?;
    Ok(input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_time() {
        // Valid RFC 3339 cases
        assert!(validate_time("2023-12-25T10:30:00Z").is_ok());
        assert!(validate_time("2023-12-25T10:30:00+00:00").is_ok());
        assert!(validate_time("2023-12-25T10:30:00-05:00").is_ok());
        
        // Valid datetime cases
        assert!(validate_time("2023-12-25 10:30:00").is_ok());
        
        // Valid date cases
        assert!(validate_time("2023-12-25").is_ok());
        
        // Invalid cases
        assert_eq!(validate_time("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_time("invalid-date").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_time("2023-13-25").err(), Some(ValidationError::ParseFailure)); // Invalid month
        assert_eq!(validate_time("2023-12-32").err(), Some(ValidationError::ParseFailure)); // Invalid day
    }

    #[test]
    fn test_validate_unix_timestamp() {
        // Valid cases
        assert!(validate_unix_timestamp("1703505000").is_ok()); // 2023-12-25 10:30:00 UTC
        assert!(validate_unix_timestamp("0").is_ok()); // Unix epoch
        
        // Invalid cases
        assert_eq!(validate_unix_timestamp("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_unix_timestamp("abc").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_unix_timestamp("12.34").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_time_comprehensive() {
        // Valid RFC 3339 cases
        assert!(validate_time_comprehensive("2023-12-25T10:30:00Z").is_ok());
        
        // Valid datetime cases
        assert!(validate_time_comprehensive("2023-12-25 10:30:00").is_ok());
        
        // Valid date cases
        assert!(validate_time_comprehensive("2023-12-25").is_ok());
        
        // Valid Unix timestamp cases
        assert!(validate_time_comprehensive("1703505000").is_ok());
        
        // Invalid cases
        assert_eq!(validate_time_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_time_comprehensive("invalid-date").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_time_comprehensive("2023-13-25").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_time_string() {
        // Valid cases
        assert_eq!(validate_time_string("2023-12-25T10:30:00Z"), Ok("2023-12-25T10:30:00Z".to_string()));
        assert_eq!(validate_time_string("1703505000"), Ok("1703505000".to_string()));
        
        // Invalid cases
        assert_eq!(validate_time_string("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_time_string("invalid-date").err(), Some(ValidationError::ParseFailure));
    }
}