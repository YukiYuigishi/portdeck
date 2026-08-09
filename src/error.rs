//! Application-level error representation.

use thiserror::Error;

use crate::application::{ManagerError, RuleError};
use crate::cli::CliError;
use crate::config::{ConfigError, StoreError};
use crate::logging::LoggingError;
use crate::runtime::RuntimeError;
use crate::ssh::SshError;
use crate::tui::TuiError;

/// Error returned by portdeck application services.
#[derive(Debug, Error)]
pub enum AppError {
    /// DEBUG file setup or tracing subscriber installation failed.
    #[error(transparent)]
    Logging(#[from] LoggingError),
    /// SSH target discovery failed.
    #[error(transparent)]
    Configuration(#[from] ConfigError),
    /// Persistent portdeck configuration failed to load or save.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Runtime directory setup failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    /// Session manager operation failed before entering the TUI.
    #[error(transparent)]
    Manager(#[from] ManagerError),
    /// Saved rule catalog initialization failed.
    #[error(transparent)]
    Rule(#[from] RuleError),
    /// Direct OpenSSH diagnostic execution failed.
    #[error(transparent)]
    OpenSsh(#[from] SshError),
    /// OpenSSH diagnostic process returned failure.
    #[error("OpenSSH diagnostic failed: {0}")]
    OpenSshDiagnostic(String),
    /// Terminal UI setup, operation, or restoration failed.
    #[error(transparent)]
    Tui(#[from] TuiError),
    /// One or more owned ControlMasters could not be stopped on exit.
    #[error("failed to stop owned SSH sessions: {0}")]
    Shutdown(String),
    /// Unsupported command-line input.
    #[error(transparent)]
    Cli(#[from] CliError),
}

/// Result type used by portdeck application services.
pub type Result<T> = std::result::Result<T, AppError>;
