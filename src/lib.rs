//! Core library for the portdeck application.
//!
//! The crate keeps domain state, OpenSSH process control, configuration,
//! runtime resources, and terminal presentation in separate modules.

pub mod application;
pub mod cli;
pub mod config;
pub mod domain;
pub mod error;
pub mod logging;
mod platform;
pub mod runtime;
pub mod ssh;
pub mod tui;
