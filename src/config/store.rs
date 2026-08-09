use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{ForwardKind, ForwardRule, ForwardRuleId, Target, TargetId};
use crate::ssh::{DynamicForwardSpec, LocalForwardSpec, validate_host_alias};

const SCHEMA_VERSION: u32 = 1;

/// Atomic TOML store for saved forward definitions.
#[derive(Debug)]
pub struct RuleStore {
    path: PathBuf,
    preserved_unknown_targets: Vec<StoredTarget>,
}

impl RuleStore {
    /// Resolves `$XDG_CONFIG_HOME/portdeck/config.toml` with the XDG fallback.
    pub fn from_environment() -> Result<Self, StoreError> {
        let base =
            if let Some(path) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(StoreError::RelativeConfigHome(path));
                }
                path
            } else {
                let home = env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .ok_or(StoreError::HomeDirectoryUnavailable)?;
                home.join(".config")
            };
        Ok(Self::at(base.join("portdeck/config.toml")))
    }

    /// Uses an explicit file path, primarily for isolated tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            preserved_unknown_targets: Vec::new(),
        }
    }

    /// Configuration file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads known target rules while preserving unknown target sections for later writes.
    pub fn load(&mut self, targets: &[Target]) -> Result<Vec<ForwardRule>, StoreError> {
        let operation_id = crate::logging::next_operation_id();
        tracing::debug!(
            component = "persistence",
            operation = "load",
            operation_id,
            targets = targets.len(),
            "saved forward load started"
        );
        let result = self.load_inner(targets);
        tracing::debug!(
            component = "persistence",
            operation = "load",
            operation_id,
            success = result.is_ok(),
            rules = result.as_ref().map_or(0, Vec::len),
            error_kind = result.as_ref().err().map(store_error_kind),
            "saved forward load completed"
        );
        result
    }

    fn load_inner(&mut self, targets: &[Target]) -> Result<Vec<ForwardRule>, StoreError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.preserved_unknown_targets.clear();
                return Ok(Vec::new());
            }
            Err(source) => {
                return Err(StoreError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let stored: StoredConfig =
            toml::from_str(&contents).map_err(|source| StoreError::Parse {
                path: self.path.clone(),
                source,
            })?;
        if stored.version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedVersion(stored.version));
        }

        let known_targets = targets
            .iter()
            .map(|target| (target.host_alias.to_ascii_lowercase(), target))
            .collect::<HashMap<_, _>>();
        let mut seen_target_aliases = HashSet::new();
        let mut seen_rule_ids = HashSet::new();
        let mut rules = Vec::new();
        self.preserved_unknown_targets.clear();

        for stored_target in stored.targets {
            validate_host_alias(&stored_target.host_alias).map_err(|error| {
                StoreError::InvalidData(format!(
                    "invalid stored target {:?}: {error}",
                    stored_target.host_alias
                ))
            })?;
            let target_key = stored_target.host_alias.to_ascii_lowercase();
            if !seen_target_aliases.insert(target_key.clone()) {
                return Err(StoreError::InvalidData(format!(
                    "duplicate target section: {:?}",
                    stored_target.host_alias
                )));
            }

            if let Some(target) = known_targets.get(&target_key) {
                for stored_rule in stored_target.forwards {
                    let rule = stored_rule.into_domain(&target.id)?;
                    if !seen_rule_ids.insert(rule.id.clone()) {
                        return Err(StoreError::InvalidData(format!(
                            "duplicate rule ID: {:?}",
                            rule.id.as_str()
                        )));
                    }
                    rules.push(rule);
                }
            } else {
                for stored_rule in &stored_target.forwards {
                    stored_rule.validate()?;
                }
                self.preserved_unknown_targets.push(stored_target);
            }
        }

        Ok(rules)
    }

    /// Atomically writes all known definitions without persisting runtime state.
    pub fn save(&mut self, targets: &[Target], rules: &[&ForwardRule]) -> Result<(), StoreError> {
        let operation_id = crate::logging::next_operation_id();
        tracing::debug!(
            component = "persistence",
            operation = "save",
            operation_id,
            targets = targets.len(),
            rules = rules.len(),
            "saved forward update started"
        );
        let result = self.save_inner(targets, rules);
        tracing::debug!(
            component = "persistence",
            operation = "save",
            operation_id,
            success = result.is_ok(),
            error_kind = result.as_ref().err().map(store_error_kind),
            "saved forward update completed"
        );
        result
    }

    fn save_inner(&mut self, targets: &[Target], rules: &[&ForwardRule]) -> Result<(), StoreError> {
        let mut grouped: BTreeMap<&TargetId, Vec<&ForwardRule>> = BTreeMap::new();
        for rule in rules {
            grouped.entry(&rule.target_id).or_default().push(*rule);
        }

        let mut stored_targets = Vec::new();
        for target in targets {
            let forwards = grouped
                .remove(&target.id)
                .unwrap_or_default()
                .into_iter()
                .map(StoredForward::from)
                .collect::<Vec<_>>();
            if !forwards.is_empty() {
                stored_targets.push(StoredTarget {
                    host_alias: target.host_alias.clone(),
                    forwards,
                });
            }
        }
        stored_targets.extend(self.preserved_unknown_targets.clone());

        let document = StoredConfig {
            version: SCHEMA_VERSION,
            targets: stored_targets,
        };
        let contents = toml::to_string_pretty(&document).map_err(StoreError::Serialize)?;
        atomic_write(&self.path, contents.as_bytes())
    }
}

fn store_error_kind(error: &StoreError) -> &'static str {
    match error {
        StoreError::RelativeConfigHome(_) => "relative_config_home",
        StoreError::HomeDirectoryUnavailable => "home_unavailable",
        StoreError::Read { .. } => "read",
        StoreError::Parse { .. } => "parse",
        StoreError::UnsupportedVersion(_) => "unsupported_version",
        StoreError::InvalidData(_) => "invalid_data",
        StoreError::Serialize(_) => "serialize",
        StoreError::CreateDirectory { .. } => "create_directory",
        StoreError::CreateTemporary { .. } => "create_temporary",
        StoreError::WriteTemporary { .. } => "write_temporary",
        StoreError::Replace { .. } => "replace",
        StoreError::SyncDirectory { .. } => "sync_directory",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConfig {
    version: u32,
    #[serde(default)]
    targets: Vec<StoredTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTarget {
    host_alias: String,
    #[serde(default)]
    forwards: Vec<StoredForward>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredForward {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    bind_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_local_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<StoredForwardKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum StoredForwardKind {
    Local,
    Socks,
}

impl StoredForward {
    fn validate(&self) -> Result<(), StoreError> {
        if self.id.is_empty()
            || self.id.contains(['\0', '\n', '\r'])
            || self
                .label
                .as_ref()
                .is_some_and(|label| label.contains(['\0', '\n', '\r']))
        {
            return Err(StoreError::InvalidData(format!(
                "invalid stored forward ID or label: {:?}",
                self.id
            )));
        }
        self.domain_kind().map_err(|error| {
            StoreError::InvalidData(format!("invalid stored forward {:?}: {error}", self.id))
        })?;
        Ok(())
    }

    fn domain_kind(&self) -> Result<ForwardKind, String> {
        match self.kind.unwrap_or(StoredForwardKind::Local) {
            StoredForwardKind::Local => {
                let remote_host = self
                    .remote_host
                    .as_deref()
                    .ok_or("Local forward is missing remote_host")?;
                let remote_port = self
                    .remote_port
                    .ok_or("Local forward is missing remote_port")?;
                LocalForwardSpec::new(
                    &self.bind_address,
                    self.requested_local_port.unwrap_or(remote_port),
                    remote_host,
                    remote_port,
                )
                .map_err(|error| error.to_string())?;
                Ok(ForwardKind::Local {
                    remote_host: remote_host.to_owned(),
                    remote_port,
                })
            }
            StoredForwardKind::Socks => {
                if self.remote_host.is_some() || self.remote_port.is_some() {
                    return Err("SOCKS forward must not contain a remote destination".to_owned());
                }
                DynamicForwardSpec::new(
                    &self.bind_address,
                    self.requested_local_port.unwrap_or(1080),
                )
                .map_err(|error| error.to_string())?;
                Ok(ForwardKind::Socks)
            }
        }
    }

    fn into_domain(self, target_id: &TargetId) -> Result<ForwardRule, StoreError> {
        self.validate()?;
        let kind = self.domain_kind().map_err(|error| {
            StoreError::InvalidData(format!("invalid stored forward {:?}: {error}", self.id))
        })?;
        Ok(ForwardRule {
            id: ForwardRuleId::new(self.id),
            target_id: target_id.clone(),
            label: self.label,
            bind_address: self.bind_address,
            requested_local_port: self.requested_local_port,
            kind,
        })
    }
}

impl From<&ForwardRule> for StoredForward {
    fn from(rule: &ForwardRule) -> Self {
        let (kind, remote_host, remote_port) = match &rule.kind {
            ForwardKind::Local {
                remote_host,
                remote_port,
            } => (
                StoredForwardKind::Local,
                Some(remote_host.clone()),
                Some(*remote_port),
            ),
            ForwardKind::Socks => (StoredForwardKind::Socks, None, None),
        };
        Self {
            id: rule.id.as_str().to_owned(),
            label: rule.label.clone(),
            bind_address: rule.bind_address.clone(),
            requested_local_port: rule.requested_local_port,
            kind: Some(kind),
            remote_host,
            remote_port,
        }
    }
}

/// Persistent configuration read, validation, or atomic-write failure.
#[derive(Debug, Error)]
pub enum StoreError {
    /// XDG_CONFIG_HOME must be absolute when set.
    #[error("XDG_CONFIG_HOME must be absolute: {0}")]
    RelativeConfigHome(PathBuf),
    /// HOME is required when XDG_CONFIG_HOME is absent.
    #[error("HOME is not set; cannot locate the portdeck configuration")]
    HomeDirectoryUnavailable,
    /// Existing configuration could not be read.
    #[error("failed to read portdeck configuration {path}: {source}")]
    Read {
        /// Configuration path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// TOML was malformed or did not match the schema.
    #[error("failed to parse portdeck configuration {path}: {source}")]
    Parse {
        /// Configuration path.
        path: PathBuf,
        /// TOML diagnostic.
        #[source]
        source: toml::de::Error,
    },
    /// Configuration used a future or unsupported schema.
    #[error("unsupported portdeck configuration version: {0}")]
    UnsupportedVersion(u32),
    /// Parsed values were unsafe or inconsistent.
    #[error("invalid portdeck configuration: {0}")]
    InvalidData(String),
    /// Domain values could not be serialized.
    #[error("failed to serialize portdeck configuration: {0}")]
    Serialize(toml::ser::Error),
    /// Parent configuration directory could not be created.
    #[error("failed to create configuration directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Temporary file could not be created.
    #[error("failed to create temporary configuration {path}: {source}")]
    CreateTemporary {
        /// Temporary path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Temporary file write or sync failed.
    #[error("failed to write temporary configuration {path}: {source}")]
    WriteTemporary {
        /// Temporary path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Atomic rename failed.
    #[error("failed to replace configuration {path}: {source}")]
    Replace {
        /// Final configuration path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Parent directory sync failed after rename.
    #[error("failed to sync configuration directory {path}: {source}")]
    SyncDirectory {
        /// Directory path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
}

struct TemporaryFile {
    path: PathBuf,
    committed: bool,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    let parent = path.parent().ok_or_else(|| {
        StoreError::InvalidData(format!(
            "configuration path has no parent: {}",
            path.display()
        ))
    })?;
    if !parent.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(parent)
            .map_err(|source| StoreError::CreateDirectory {
                path: parent.to_owned(),
                source,
            })?;
    }

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary_path = path.with_extension(format!("tmp-{}-{unique}", std::process::id()));
    let mut temporary = TemporaryFile {
        path: temporary_path.clone(),
        committed: false,
    };
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary_path)
        .map_err(|source| StoreError::CreateTemporary {
            path: temporary_path.clone(),
            source,
        })?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|source| StoreError::WriteTemporary {
            path: temporary_path.clone(),
            source,
        })?;
    drop(file);

    fs::rename(&temporary_path, path).map_err(|source| StoreError::Replace {
        path: path.to_owned(),
        source,
    })?;
    temporary.committed = true;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| StoreError::SyncDirectory {
            path: parent.to_owned(),
            source,
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::domain::{ForwardKind, ForwardRule, ForwardRuleId, Target, TargetId};

    use super::{RuleStore, StoreError};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "portdeck-store-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn target(alias: &str) -> Target {
        Target {
            id: TargetId::new(alias.to_ascii_lowercase()),
            host_alias: alias.to_owned(),
            source: PathBuf::from("config"),
        }
    }

    fn rule() -> ForwardRule {
        ForwardRule {
            id: ForwardRuleId::new("rule-1"),
            target_id: TargetId::new("dev"),
            label: Some("database".to_owned()),
            bind_address: "127.0.0.1".to_owned(),
            requested_local_port: None,
            kind: ForwardKind::Local {
                remote_host: "db.internal".to_owned(),
                remote_port: 5432,
            },
        }
    }

    #[test]
    fn missing_file_loads_as_empty_configuration() {
        let directory = TestDirectory::new();
        let mut store = RuleStore::at(directory.path().join("config.toml"));

        assert!(store.load(&[target("dev")]).unwrap().is_empty());
    }

    #[test]
    fn rules_round_trip_without_runtime_fields() {
        let directory = TestDirectory::new();
        let path = directory.path().join("nested/config.toml");
        let mut store = RuleStore::at(&path);
        let targets = [target("dev")];
        let saved = rule();

        store.save(&targets, &[&saved]).unwrap();
        let loaded = store.load(&targets).unwrap();

        assert_eq!(loaded, [saved]);
        let contents = fs::read_to_string(path).unwrap();
        assert!(!contents.contains("actual_local_port"));
        assert!(!contents.contains("session"));
        assert!(!contents.contains("pid"));
        assert!(!contents.contains("passphrase"));
    }

    #[test]
    fn local_and_socks_rules_round_trip_with_explicit_kinds() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let mut store = RuleStore::at(&path);
        let local = rule();
        let socks = ForwardRule {
            id: ForwardRuleId::new("proxy"),
            target_id: TargetId::new("dev"),
            label: Some("browser".to_owned()),
            bind_address: "::1".to_owned(),
            requested_local_port: Some(1081),
            kind: ForwardKind::Socks,
        };

        store.save(&[target("dev")], &[&local, &socks]).unwrap();
        let loaded = store.load(&[target("dev")]).unwrap();

        assert_eq!(loaded, [local, socks]);
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("kind = \"local\""));
        assert!(contents.contains("kind = \"socks\""));
    }

    #[test]
    fn legacy_rule_without_kind_migrates_to_local() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"version = 1

[[targets]]
host_alias = "dev"

[[targets.forwards]]
id = "legacy-web"
bind_address = "127.0.0.1"
remote_host = "127.0.0.1"
remote_port = 3000
"#,
        )
        .unwrap();
        let mut store = RuleStore::at(&path);

        let loaded = store.load(&[target("dev")]).unwrap();

        assert_eq!(
            loaded[0].kind,
            ForwardKind::Local {
                remote_host: "127.0.0.1".to_owned(),
                remote_port: 3000,
            }
        );
        store.save(&[target("dev")], &[&loaded[0]]).unwrap();
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .contains("kind = \"local\"")
        );
    }

    #[test]
    fn socks_rule_rejects_remote_destination_fields() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"version = 1

[[targets]]
host_alias = "dev"

[[targets.forwards]]
id = "proxy"
bind_address = "127.0.0.1"
kind = "socks"
remote_host = "unexpected"
remote_port = 80
"#,
        )
        .unwrap();

        assert!(matches!(
            RuleStore::at(path).load(&[target("dev")]),
            Err(StoreError::InvalidData(_))
        ));
    }

    #[test]
    fn edited_rule_replaces_persisted_fields_without_changing_identity() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let targets = [target("dev")];
        let mut store = RuleStore::at(&path);
        let original = rule();
        store.save(&targets, &[&original]).unwrap();
        let mut edited = original.clone();
        edited.label = Some("edited database".to_owned());
        edited.bind_address = "::1".to_owned();
        edited.requested_local_port = Some(15432);
        edited.kind = ForwardKind::Local {
            remote_host: "db.internal.example".to_owned(),
            remote_port: 6432,
        };

        store.save(&targets, &[&edited]).unwrap();
        let loaded = store.load(&targets).unwrap();

        assert_eq!(loaded, [edited]);
        assert_eq!(loaded[0].id, original.id);
        assert_eq!(loaded[0].target_id, original.target_id);
    }

    #[test]
    fn preserves_rules_for_temporarily_unknown_targets() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        let contents = r#"
version = 1

[[targets]]
host_alias = "offline"

[[targets.forwards]]
id = "offline-rule"
bind_address = "127.0.0.1"
remote_host = "127.0.0.1"
remote_port = 3000
"#;
        fs::write(&path, contents).unwrap();
        let mut store = RuleStore::at(&path);
        store.load(&[target("dev")]).unwrap();

        let known = rule();
        store.save(&[target("dev")], &[&known]).unwrap();

        let rewritten = fs::read_to_string(path).unwrap();
        assert!(rewritten.contains("offline-rule"));
        assert!(rewritten.contains("rule-1"));
    }

    #[test]
    fn malformed_configuration_fails_without_rewriting_it() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        fs::write(&path, "not valid = [toml").unwrap();
        let original = fs::read_to_string(&path).unwrap();
        let mut store = RuleStore::at(&path);

        assert!(matches!(
            store.load(&[target("dev")]),
            Err(StoreError::Parse { .. })
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn rejects_runtime_or_unknown_schema_fields() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "version = 1\nactual_local_port = 8080\ntargets = []\n",
        )
        .unwrap();
        let mut store = RuleStore::at(path);

        assert!(matches!(
            store.load(&[target("dev")]),
            Err(StoreError::Parse { .. })
        ));
    }
}
