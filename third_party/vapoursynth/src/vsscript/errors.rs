use std::ffi::{CString, NulError};
use std::{fmt, io};

use thiserror::Error;

/// The error type for `vsscript` operations.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Couldn't convert to a CString")]
    CStringConversion(#[source] NulError),
    #[error("Couldn't open the file")]
    FileOpen(#[source] io::Error),
    #[error("Couldn't read the file")]
    FileRead(#[source] io::Error),
    #[error("Path isn't valid Unicode")]
    PathInvalidUnicode,
    #[error("An error occurred in VSScript")]
    VSScript(#[source] VSScriptError),
    #[error("There's no such variable")]
    NoSuchVariable,
    #[error("Couldn't get the core")]
    NoCore,
    #[error("There's no output on the requested index")]
    NoOutput,
    #[error("Couldn't get the VapourSynth API")]
    NoAPI,
    #[error("Failed to create VSScript environment")]
    ScriptCreationFailed,
}

impl From<NulError> for Error {
    #[inline]
    fn from(x: NulError) -> Self {
        Error::CStringConversion(x)
    }
}

impl From<VSScriptError> for Error {
    #[inline]
    fn from(x: VSScriptError) -> Self {
        Error::VSScript(x)
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// A container for a VSScript error.
#[derive(Error, Debug)]
pub struct VSScriptError(CString);

impl fmt::Display for VSScriptError {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.to_string_lossy())
    }
}

impl VSScriptError {
    /// Creates a new `VSScriptError` with the given error message.
    #[inline]
    pub(crate) fn new(message: CString) -> Self {
        VSScriptError(message)
    }
}
