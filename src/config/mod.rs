//! SSH target discovery and portdeck-owned persistent configuration.
//!
//! OpenSSH remains responsible for resolving the effective SSH configuration.

mod targets;

pub use targets::{ConfigError, EffectiveSshConfig, TargetDiscovery, parse_effective_config};
