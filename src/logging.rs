//! Structured logging initialization.

use crate::error::{AppError, Result};

/// Installs portdeck's process-wide tracing subscriber.
pub fn init() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .try_init()
        .map_err(AppError::LoggingInitialization)
}
