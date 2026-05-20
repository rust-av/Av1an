use std::borrow::Cow;
use std::error::Error;
use std::ffi::CStr;
use std::fmt;

/// A container for a `get_frame` error.
#[derive(Debug)]
pub struct GetFrameError<'a>(Cow<'a, CStr>);

impl<'a> fmt::Display for GetFrameError<'a> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.to_string_lossy())
    }
}

impl<'a> Error for GetFrameError<'a> {
    #[inline]
    fn description(&self) -> &str {
        "VapourSynth error"
    }
}

impl<'a> GetFrameError<'a> {
    /// Creates a new `GetFrameError` with the given error message.
    #[inline]
    pub(crate) fn new(message: Cow<'a, CStr>) -> Self {
        GetFrameError(message)
    }

    /// Consumes this error, returning its underlying error message.
    #[inline]
    pub fn into_inner(self) -> Cow<'a, CStr> {
        self.0
    }
}
