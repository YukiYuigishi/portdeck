use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use glob::{PatternError, glob};
use thiserror::Error;

use crate::domain::{Target, TargetId};

/// Discovers concrete SSH `Host` aliases without interpreting effective options.
#[derive(Debug, Clone)]
pub struct TargetDiscovery {
    root_config: PathBuf,
    user_ssh_directory: PathBuf,
    home_directory: PathBuf,
}

impl TargetDiscovery {
    /// Uses `$HOME/.ssh/config` and `$HOME/.ssh` Include semantics.
    pub fn from_environment() -> Result<Self, ConfigError> {
        let home_directory = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(ConfigError::HomeDirectoryUnavailable)?;
        Ok(Self::for_home(home_directory))
    }

    /// Uses a supplied home directory, primarily for isolated tests.
    pub fn for_home(home_directory: impl Into<PathBuf>) -> Self {
        let home_directory = home_directory.into();
        let user_ssh_directory = home_directory.join(".ssh");
        let root_config = user_ssh_directory.join("config");
        Self {
            root_config,
            user_ssh_directory,
            home_directory,
        }
    }

    /// Uses explicit paths while preserving user-config Include rules.
    pub fn with_paths(
        root_config: impl Into<PathBuf>,
        user_ssh_directory: impl Into<PathBuf>,
        home_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            root_config: root_config.into(),
            user_ssh_directory: user_ssh_directory.into(),
            home_directory: home_directory.into(),
        }
    }

    /// Lists concrete aliases from the root file and recursively included files.
    pub fn discover(&self) -> Result<Vec<Target>, ConfigError> {
        if !self.root_config.exists() {
            return Ok(Vec::new());
        }

        let mut parser = TargetParser {
            user_ssh_directory: &self.user_ssh_directory,
            home_directory: &self.home_directory,
            visited_files: HashSet::new(),
            seen_aliases: HashSet::new(),
            targets: Vec::new(),
        };
        parser.parse_file(&self.root_config)?;
        Ok(parser.targets)
    }
}

/// Display-oriented fields parsed from `ssh -G` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSshConfig {
    /// Effective destination hostname.
    pub hostname: String,
    /// Effective remote user.
    pub user: String,
    /// Effective SSH server port.
    pub port: u16,
    /// Effective ProxyJump expression when configured.
    pub proxy_jump: Option<String>,
}

/// Parses selected display fields from successful `ssh -G` output.
pub fn parse_effective_config(output: &str) -> Result<EffectiveSshConfig, ConfigError> {
    let mut hostname = None;
    let mut user = None;
    let mut port = None;
    let mut proxy_jump = None;

    for line in output.lines() {
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()),
            "port" => {
                port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| ConfigError::InvalidEffectivePort(value.to_owned()))?,
                );
            }
            "proxyjump" if value != "none" => proxy_jump = Some(value.to_owned()),
            _ => {}
        }
    }

    Ok(EffectiveSshConfig {
        hostname: hostname.ok_or(ConfigError::MissingEffectiveField("hostname"))?,
        user: user.ok_or(ConfigError::MissingEffectiveField("user"))?,
        port: port.ok_or(ConfigError::MissingEffectiveField("port"))?,
        proxy_jump,
    })
}

struct TargetParser<'a> {
    user_ssh_directory: &'a Path,
    home_directory: &'a Path,
    visited_files: HashSet<PathBuf>,
    seen_aliases: HashSet<String>,
    targets: Vec<Target>,
}

impl TargetParser<'_> {
    fn parse_file(&mut self, path: &Path) -> Result<(), ConfigError> {
        let canonical_path = path.canonicalize().map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        if !self.visited_files.insert(canonical_path.clone()) {
            return Ok(());
        }

        let contents = fs::read_to_string(&canonical_path).map_err(|source| ConfigError::Read {
            path: canonical_path.clone(),
            source,
        })?;

        for (line_index, line) in contents.lines().enumerate() {
            let Some((keyword, arguments)) =
                split_directive(line).map_err(|message| ConfigError::Syntax {
                    path: canonical_path.clone(),
                    line: line_index + 1,
                    message,
                })?
            else {
                continue;
            };

            if keyword.eq_ignore_ascii_case("host") {
                for alias in arguments {
                    if is_concrete_alias(&alias) {
                        let deduplication_key = alias.to_ascii_lowercase();
                        if self.seen_aliases.insert(deduplication_key.clone()) {
                            self.targets.push(Target {
                                id: TargetId::new(deduplication_key),
                                host_alias: alias,
                                source: canonical_path.clone(),
                            });
                        }
                    }
                }
            } else if keyword.eq_ignore_ascii_case("include") {
                for pattern in arguments {
                    for included_path in self.expand_include(&pattern)? {
                        self.parse_file(&included_path)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn expand_include(&self, pattern: &str) -> Result<Vec<PathBuf>, ConfigError> {
        let expanded = if pattern == "~" {
            self.home_directory.to_owned()
        } else if let Some(suffix) = pattern.strip_prefix("~/") {
            self.home_directory.join(suffix)
        } else if pattern.starts_with('~') {
            return Err(ConfigError::UnsupportedTildeUser(pattern.to_owned()));
        } else {
            let path = PathBuf::from(pattern);
            if path.is_absolute() {
                path
            } else {
                self.user_ssh_directory.join(path)
            }
        };

        let pattern_text = expanded.to_string_lossy();
        let paths = glob(&pattern_text)
            .map_err(|source| ConfigError::InvalidIncludePattern {
                pattern: pattern.to_owned(),
                source,
            })?
            .filter_map(Result::ok)
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();

        let mut paths = paths;
        paths.sort();
        Ok(paths)
    }
}

/// SSH target discovery or effective-config parsing failure.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// `$HOME` was absent or empty.
    #[error("HOME is not set; cannot locate the user SSH configuration")]
    HomeDirectoryUnavailable,
    /// A configuration file could not be canonicalized or read.
    #[error("failed to read SSH configuration {path}: {source}")]
    Read {
        /// File being read.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// A relevant directive used invalid quoting or escaping.
    #[error("invalid SSH configuration syntax at {path}:{line}: {message}")]
    Syntax {
        /// File containing the directive.
        path: PathBuf,
        /// One-based line number.
        line: usize,
        /// Parser diagnostic.
        message: String,
    },
    /// An Include glob was syntactically invalid.
    #[error("invalid SSH Include pattern {pattern:?}: {source}")]
    InvalidIncludePattern {
        /// Original pattern.
        pattern: String,
        /// Glob parser error.
        #[source]
        source: PatternError,
    },
    /// `~other-user` cannot be resolved without changing OpenSSH semantics.
    #[error("Include path {0:?} uses an unsupported named-user tilde")]
    UnsupportedTildeUser(String),
    /// `ssh -G` omitted a field expected from OpenSSH defaults.
    #[error("ssh -G output did not contain {0}")]
    MissingEffectiveField(&'static str),
    /// `ssh -G` returned a port outside the supported numeric form.
    #[error("ssh -G returned an invalid port: {0:?}")]
    InvalidEffectivePort(String),
}

fn is_concrete_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('!')
        && !alias.contains(['*', '?'])
        && !alias.starts_with('-')
        && !alias.chars().any(char::is_whitespace)
        && !alias.contains(['\0', '\n', '\r'])
}

fn split_directive(line: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }

    let keyword_end = line
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(line.len());
    let keyword = &line[..keyword_end];
    if keyword.is_empty() {
        return Ok(None);
    }

    let arguments = line[keyword_end..]
        .trim_start()
        .strip_prefix('=')
        .unwrap_or(&line[keyword_end..])
        .trim_start();
    let arguments = tokenize_arguments(arguments)?;
    Ok(Some((keyword.to_owned(), arguments)))
}

fn tokenize_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut token_started = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            token_started = true;
            continue;
        }
        if character == '\\' {
            escaped = true;
            token_started = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                current.push(character);
            }
            token_started = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            token_started = true;
        } else if character == '#' {
            break;
        } else if character.is_whitespace() {
            if token_started {
                tokens.push(std::mem::take(&mut current));
                token_started = false;
            }
        } else {
            current.push(character);
            token_started = true;
        }
    }

    if escaped {
        return Err("trailing escape".to_owned());
    }
    if quote.is_some() {
        return Err("unterminated quote".to_owned());
    }
    if token_started {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{TargetDiscovery, parse_effective_config, split_directive};

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "portdeck-config-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(path.join(".ssh/includes")).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, contents: &str) {
            fs::write(self.0.join(relative), contents).unwrap();
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn missing_root_config_is_an_empty_catalog() {
        let home = TestHome::new();

        let targets = TargetDiscovery::for_home(home.path()).discover().unwrap();

        assert!(targets.is_empty());
    }

    #[test]
    fn discovers_concrete_hosts_from_recursive_lexical_includes() {
        let home = TestHome::new();
        home.write(
            ".ssh/config",
            "Include includes/*.conf\nHost *\n  User default\nHost root-host duplicate\n",
        );
        home.write(
            ".ssh/includes/a.conf",
            "Include includes/b.conf\nHost alpha duplicate wildcard-* !negative\n",
        );
        home.write(
            ".ssh/includes/b.conf",
            "Include config\nHost beta question?\n",
        );

        let targets = TargetDiscovery::for_home(home.path()).discover().unwrap();
        let aliases = targets
            .iter()
            .map(|target| target.host_alias.as_str())
            .collect::<Vec<_>>();

        assert_eq!(aliases, ["beta", "alpha", "duplicate", "root-host"]);
        assert!(targets.iter().all(|target| target.source.is_absolute()));
    }

    #[test]
    fn supports_quoted_include_paths_and_equals_directives() {
        let home = TestHome::new();
        fs::create_dir(home.path().join(".ssh/with space")).unwrap();
        home.write(
            ".ssh/config",
            "Include = \"with space/targets.conf\"\nHost=main # comment\n",
        );
        home.write(".ssh/with space/targets.conf", "Host included\n");

        let targets = TargetDiscovery::for_home(home.path()).discover().unwrap();
        let aliases = targets
            .iter()
            .map(|target| target.host_alias.as_str())
            .collect::<Vec<_>>();

        assert_eq!(aliases, ["included", "main"]);
    }

    #[test]
    fn parses_display_fields_from_openssh_output() {
        let config = parse_effective_config(
            "host dev\nhostname dev.example.test\nuser alice\nport 2222\nproxyjump bastion\n",
        )
        .unwrap();

        assert_eq!(config.hostname, "dev.example.test");
        assert_eq!(config.user, "alice");
        assert_eq!(config.port, 2222);
        assert_eq!(config.proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn tokenizes_comments_quotes_and_escapes_without_a_shell() {
        let (_, arguments) = split_directive("Host 'quoted host' escaped\\ value # ignored")
            .unwrap()
            .unwrap();

        assert_eq!(arguments, ["quoted host", "escaped value"]);
    }
}
