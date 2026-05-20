use std::ffi::NulError;
use std::result;

use thiserror::Error;

/// The error type for `Map` operations.
#[derive(Error, Debug, Eq, PartialEq)]
pub enum Error {
    #[error("The requested key wasn't found in the map")]
    KeyNotFound,
    #[error("The requested index was out of bounds")]
    IndexOutOfBounds,
    #[error("The given/requested value type doesn't match the type of the property")]
    WrongValueType,
    #[error("The key is invalid")]
    InvalidKey(#[from] InvalidKeyError),
    #[error("Couldn't convert to a CString")]
    CStringConversion(#[from] NulError),
    #[error("An unexpected error occurred")]
    UnknownError,
}

/// A specialized `Result` type for `Map` operations.
pub type Result<T> = result::Result<T, Error>;

/// An error indicating the map key is invalid.
#[derive(Error, Debug, Eq, PartialEq)]
#[rustfmt::skip]
pub enum InvalidKeyError {
    #[error("The key is empty")]
    EmptyKey,
    #[error("The key contains an invalid character at index {}", _0)]
    InvalidCharacter(usize),
}
