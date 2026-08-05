//! SSH target discovery and portdeck-owned persistent configuration.
//!
//! OpenSSH remains responsible for resolving the effective SSH configuration.

mod store;
mod targets;

pub use store::{RuleStore, StoreError};
pub use targets::{ConfigError, EffectiveSshConfig, TargetDiscovery, parse_effective_config};
