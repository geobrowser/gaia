/// Functions for validating point (coordinate) strings.

use super::error::ValidationError;

/// Represents a 2D point with x and y coordinates
#[derive(Debug, PartialEq, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Validates if the input string represents a valid point in various formats.
/// 
/// Supported formats:
/// - "x,y" (comma separated)
/// - "(x,y)" (with parentheses)
/// - "x y" (space separated)
/// - "{\"x\":1.0,\"y\":2.0}" (JSON format)
///
/// # Arguments
///
/// * `input` - A string slice that contains the point to validate
///
/// # Returns
///
/// * `Ok(Point)` - If the input is a valid point
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_point(input: &str) -> Result<Point, ValidationError> {
    let trimmed = input.trim();
    
    // Try comma-separated format: "x,y"
    if let Some((x_str, y_str)) = try_parse_comma_separated(trimmed) {
        if let (Ok(x), Ok(y)) = (x_str.parse::<f64>(), y_str.parse::<f64>()) {
            return Ok(Point { x, y });
        }
    }
    
    // Try parentheses format: "(x,y)" or "(x y)"
    if let Some(inner) = try_parse_parentheses(trimmed) {
        if let Some((x_str, y_str)) = try_parse_comma_separated(inner) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<f64>(), y_str.parse::<f64>()) {
                return Ok(Point { x, y });
            }
        }
        if let Some((x_str, y_str)) = try_parse_space_separated(inner) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<f64>(), y_str.parse::<f64>()) {
                return Ok(Point { x, y });
            }
        }
    }
    
    // Try space-separated format: "x y"
    if let Some((x_str, y_str)) = try_parse_space_separated(trimmed) {
        if let (Ok(x), Ok(y)) = (x_str.parse::<f64>(), y_str.parse::<f64>()) {
            return Ok(Point { x, y });
        }
    }
    
    // Try JSON format: {"x":1.0,"y":2.0}
    if let Ok(point) = try_parse_json(trimmed) {
        return Ok(point);
    }
    
    Err(ValidationError::ParseFailure)
}

/// A more comprehensive point validator that provides specific error messages.
///
/// # Arguments
///
/// * `input` - A string slice that contains the point to validate
///
/// # Returns
///
/// * `Ok(Point)` - If the input is a valid point
/// * `Err(ValidationError)` - If the input is invalid, with a specific error type
pub fn validate_point_comprehensive(input: &str) -> Result<Point, ValidationError> {
    // Check if empty
    if input.is_empty() {
        return Err(ValidationError::EmptyInput);
    }
    
    // Use the basic validator
    validate_point(input)
}

/// Validates if the input string represents a valid point and returns the string representation.
/// 
/// This is useful when you want to validate but keep the original string format.
///
/// # Arguments
///
/// * `input` - A string slice that contains the point to validate
///
/// # Returns
///
/// * `Ok(String)` - If the input is a valid point (returns the original string)
/// * `Err(ValidationError)` - If the input is invalid
pub fn validate_point_string(input: &str) -> Result<String, ValidationError> {
    validate_point_comprehensive(input)?;
    Ok(input.to_string())
}

// Helper functions for parsing different formats

fn try_parse_comma_separated(input: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = input.split(',').collect();
    if parts.len() == 2 {
        Some((parts[0].trim(), parts[1].trim()))
    } else {
        None
    }
}

fn try_parse_space_separated(input: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() == 2 {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

fn try_parse_parentheses(input: &str) -> Option<&str> {
    if input.starts_with('(') && input.ends_with(')') && input.len() > 2 {
        Some(&input[1..input.len()-1])
    } else {
        None
    }
}

fn try_parse_json(input: &str) -> Result<Point, ValidationError> {
    // Simple JSON parsing for point format
    if !input.starts_with('{') || !input.ends_with('}') {
        return Err(ValidationError::ParseFailure);
    }
    
    let inner = &input[1..input.len()-1];
    let mut x_val: Option<f64> = None;
    let mut y_val: Option<f64> = None;
    
    // Split by comma and parse key-value pairs
    for pair in inner.split(',') {
        let pair = pair.trim();
        if let Some(colon_pos) = pair.find(':') {
            let key = pair[..colon_pos].trim().trim_matches('"');
            let value_str = pair[colon_pos+1..].trim();
            
            if let Ok(value) = value_str.parse::<f64>() {
                match key {
                    "x" => x_val = Some(value),
                    "y" => y_val = Some(value),
                    _ => {}
                }
            }
        }
    }
    
    match (x_val, y_val) {
        (Some(x), Some(y)) => Ok(Point { x, y }),
        _ => Err(ValidationError::ParseFailure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::ValidationError;

    #[test]
    fn test_validate_point() {
        // Valid comma-separated cases
        assert_eq!(validate_point("1.5,2.5"), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(validate_point("0,0"), Ok(Point { x: 0.0, y: 0.0 }));
        assert_eq!(validate_point("-1.5,2.5"), Ok(Point { x: -1.5, y: 2.5 }));
        
        // Valid parentheses cases
        assert_eq!(validate_point("(1.5,2.5)"), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(validate_point("(1.5 2.5)"), Ok(Point { x: 1.5, y: 2.5 }));
        
        // Valid space-separated cases
        assert_eq!(validate_point("1.5 2.5"), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(validate_point("0 0"), Ok(Point { x: 0.0, y: 0.0 }));
        
        // Valid JSON cases
        assert_eq!(validate_point(r#"{"x":1.5,"y":2.5}"#), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(validate_point(r#"{"y":2.5,"x":1.5}"#), Ok(Point { x: 1.5, y: 2.5 }));
        
        // Invalid cases
        assert_eq!(validate_point("").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_point("1.5").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_point("1.5,2.5,3.5").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_point("abc,def").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_point("(1.5)").err(), Some(ValidationError::ParseFailure));
        assert_eq!(validate_point("{\"x\":1.5}").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_point_comprehensive() {
        // Valid cases
        assert_eq!(validate_point_comprehensive("1.5,2.5"), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(validate_point_comprehensive("(1.5,2.5)"), Ok(Point { x: 1.5, y: 2.5 }));
        
        // Invalid cases
        assert_eq!(validate_point_comprehensive("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_point_comprehensive("invalid").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_validate_point_string() {
        // Valid cases
        assert_eq!(validate_point_string("1.5,2.5"), Ok("1.5,2.5".to_string()));
        assert_eq!(validate_point_string("(1.5,2.5)"), Ok("(1.5,2.5)".to_string()));
        
        // Invalid cases
        assert_eq!(validate_point_string("").err(), Some(ValidationError::EmptyInput));
        assert_eq!(validate_point_string("invalid").err(), Some(ValidationError::ParseFailure));
    }

    #[test]
    fn test_helper_functions() {
        assert_eq!(try_parse_comma_separated("1.5,2.5"), Some(("1.5", "2.5")));
        assert_eq!(try_parse_comma_separated("1.5"), None);
        
        assert_eq!(try_parse_space_separated("1.5 2.5"), Some(("1.5", "2.5")));
        assert_eq!(try_parse_space_separated("1.5"), None);
        
        assert_eq!(try_parse_parentheses("(1.5,2.5)"), Some("1.5,2.5"));
        assert_eq!(try_parse_parentheses("1.5,2.5"), None);
        
        assert_eq!(try_parse_json(r#"{"x":1.5,"y":2.5}"#), Ok(Point { x: 1.5, y: 2.5 }));
        assert_eq!(try_parse_json("invalid").err(), Some(ValidationError::ParseFailure));
    }
}