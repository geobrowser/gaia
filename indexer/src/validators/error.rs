//! Error types for validator functions.

use std::fmt;

/// ValidationError represents the different types of validation errors.
#[derive(Debug, PartialEq, Clone)]
pub enum ValidationError {
    /// Input is empty
    EmptyInput,
    
    /// Input contains invalid characters
    InvalidCharacters,
    
    /// Input has multiple decimal points
    MultipleDecimalPoints,
    
    /// Input cannot be parsed as the target type
    ParseFailure,
    
    /// Input is missing a required decimal point
    MissingDecimalPoint,
    
    /// Input has incorrect number of decimal places
    IncorrectDecimalPlaces(usize, usize), // (expected, found)
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::EmptyInput => write!(f, "Empty input"),
            ValidationError::InvalidCharacters => write!(f, "Contains invalid characters"),
            ValidationError::MultipleDecimalPoints => write!(f, "Multiple decimal points"),
            ValidationError::ParseFailure => write!(f, "Cannot parse as the target type"),
            ValidationError::MissingDecimalPoint => write!(f, "Expected a decimal point"),
            ValidationError::IncorrectDecimalPlaces(expected, found) => {
                write!(f, "Expected {} decimal places, found {}", expected, found)
            }
        }
    }
}

impl std::error::Error for ValidationError {}