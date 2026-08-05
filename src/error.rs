//! Application-level error representation.

use std::error::Error as StdError;

use thiserror::Error;

/// Error returned by portdeck application services.
#[derive(Debug, Error)]
pub enum AppError {
    /// Installing the process-wide tracing subscriber failed.
    #[error("failed to initialize structured logging: {0}")]
    LoggingInitialization(#[source] Box<dyn StdError + Send + Sync + 'static>),
}

/// Result type used by portdeck application services.
pub type Result<T> = std::result::Result<T, AppError>;
