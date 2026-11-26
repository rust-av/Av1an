pub mod cancellation;
pub mod interpolators;
pub mod process;
pub mod semaphore;

/// A caught thread panic. Library diagnostics retain the payload; process
/// frontends may omit it when their panic hook already reported it.
#[derive(Debug, thiserror::Error)]
#[error("{operation} panicked: {message}")]
pub struct CaughtPanic {
    pub operation: String,
    pub message: String,
}
