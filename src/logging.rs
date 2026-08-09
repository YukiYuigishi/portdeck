//! Structured logging initialization and private DEBUG log lifecycle.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tracing::Level;
use tracing_subscriber::fmt::writer::MakeWriterExt;

const LOG_DIRECTORY: &str = "portdeck";
const RETAINED_DEBUG_LOGS: usize = 10;
const MAX_REMOVALS_PER_START: usize = 64;
const CREATE_ATTEMPTS: usize = 32;
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

/// Allocates a process-local ID used to correlate one operation's events.
pub fn next_operation_id() -> u64 {
    NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Extracts the bounded public version token from `ssh -V` diagnostics.
pub fn openssh_version_token(diagnostic: &str) -> Option<&str> {
    let token = diagnostic.split_whitespace().next()?;
    (token.starts_with("OpenSSH_")
        && token.len() <= 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')))
    .then_some(token)
}

/// Installed logging mode and the optional DEBUG log path.
#[derive(Debug, Clone)]
pub struct LoggingHandle {
    debug_path: Option<PathBuf>,
}

impl LoggingHandle {
    /// Returns the private per-run DEBUG log path when enabled.
    pub fn debug_path(&self) -> Option<&Path> {
        self.debug_path.as_deref()
    }
}

/// File-backed logging setup failure.
#[derive(Debug, Error)]
pub enum LoggingError {
    /// XDG_STATE_HOME must be an absolute path.
    #[error("XDG_STATE_HOME must be absolute: {0}")]
    RelativeStateHome(PathBuf),
    /// HOME is required when XDG_STATE_HOME is absent.
    #[error("HOME is not set; cannot locate the portdeck DEBUG log directory")]
    HomeDirectoryUnavailable,
    /// Current process ownership could not be determined.
    #[error("failed to determine current user ID: {0}")]
    CurrentUser(io::Error),
    /// The log path could not be inspected.
    #[error("failed to inspect DEBUG log path {path}: {source}")]
    Inspect {
        /// Affected path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// The log path was not a real directory.
    #[error("DEBUG log path is not a directory: {0}")]
    NotDirectory(PathBuf),
    /// The log directory belongs to another user.
    #[error("DEBUG log directory {path} is owned by uid {actual}, expected uid {expected}")]
    WrongOwner {
        /// Affected directory.
        path: PathBuf,
        /// Current process UID.
        expected: u32,
        /// Directory owner UID.
        actual: u32,
    },
    /// The private directory could not be created.
    #[error("failed to create DEBUG log directory {path}: {source}")]
    CreateDirectory {
        /// Affected directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Owner-only directory permissions could not be applied.
    #[error("failed to secure DEBUG log directory {path}: {source}")]
    DirectoryPermissions {
        /// Affected directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// A unique private log file could not be created.
    #[error("failed to create DEBUG log file in {path}: {source}")]
    CreateFile {
        /// Parent directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Log file permissions could not be enforced.
    #[error("failed to secure DEBUG log file {path}: {source}")]
    FilePermissions {
        /// Affected file.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Old owned logs could not be enumerated.
    #[error("failed to list DEBUG logs in {path}: {source}")]
    ReadDirectory {
        /// Affected directory.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// A process-wide tracing subscriber was already installed or rejected.
    #[error("failed to initialize structured logging: {0}")]
    Initialize(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
}

#[derive(Debug, Clone)]
struct SharedWriter(Arc<Mutex<File>>);

impl Write for SharedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("DEBUG log lock was poisoned"))?
            .write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("DEBUG log lock was poisoned"))?
            .flush()
    }
}

/// Installs the process-wide subscriber selected by the CLI.
pub fn init(debug: bool) -> Result<LoggingHandle, LoggingError> {
    if debug {
        init_debug()
    } else {
        tracing_subscriber::fmt()
            .with_target(false)
            .compact()
            .try_init()
            .map_err(LoggingError::Initialize)?;
        Ok(LoggingHandle { debug_path: None })
    }
}

fn init_debug() -> Result<LoggingHandle, LoggingError> {
    let directory = state_directory_from_environment()?;
    let (path, file) = create_debug_log(&directory)?;
    cleanup_owned_logs(&directory, &path)?;
    let shared = Arc::new(Mutex::new(file));
    let make_writer = {
        let shared = Arc::clone(&shared);
        move || SharedWriter(Arc::clone(&shared))
    };
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_max_level(Level::DEBUG)
        .with_writer(make_writer.with_max_level(Level::DEBUG))
        .try_init()
        .map_err(LoggingError::Initialize)?;
    install_debug_panic_hook();
    Ok(LoggingHandle {
        debug_path: Some(path),
    })
}

fn state_directory_from_environment() -> Result<PathBuf, LoggingError> {
    let base = if let Some(value) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty())
    {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(LoggingError::RelativeStateHome(path));
        }
        path
    } else {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(LoggingError::HomeDirectoryUnavailable)?;
        home.join(".local/state")
    };
    Ok(base.join(LOG_DIRECTORY))
}

fn create_debug_log(directory: &Path) -> Result<(PathBuf, File), LoggingError> {
    prepare_directory(directory)?;
    for attempt in 0..CREATE_ATTEMPTS {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = directory.join(format!(
            "debug-{timestamp}{attempt:02}-{}.log",
            std::process::id()
        ));
        match OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(
                    |source| LoggingError::FilePermissions {
                        path: path.clone(),
                        source,
                    },
                )?;
                return Ok((path, file));
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(LoggingError::CreateFile {
                    path: directory.to_owned(),
                    source,
                });
            }
        }
    }
    Err(LoggingError::CreateFile {
        path: directory.to_owned(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique per-run DEBUG log filename",
        ),
    })
}

fn prepare_directory(path: &Path) -> Result<(), LoggingError> {
    if !path.is_absolute() {
        return Err(LoggingError::RelativeStateHome(path.to_owned()));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_directory(path, &metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|source| LoggingError::CreateDirectory {
                path: path.to_owned(),
                source,
            })?;
        }
        Err(source) => {
            return Err(LoggingError::Inspect {
                path: path.to_owned(),
                source,
            });
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        LoggingError::DirectoryPermissions {
            path: path.to_owned(),
            source,
        }
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|source| LoggingError::Inspect {
        path: path.to_owned(),
        source,
    })?;
    validate_directory(path, &metadata)
}

fn validate_directory(path: &Path, metadata: &fs::Metadata) -> Result<(), LoggingError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LoggingError::NotDirectory(path.to_owned()));
    }
    let expected = current_uid()?;
    if metadata.uid() != expected {
        return Err(LoggingError::WrongOwner {
            path: path.to_owned(),
            expected,
            actual: metadata.uid(),
        });
    }
    Ok(())
}

fn current_uid() -> Result<u32, LoggingError> {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(LoggingError::CurrentUser)
}

fn cleanup_owned_logs(directory: &Path, current: &Path) -> Result<(), LoggingError> {
    let entries = fs::read_dir(directory).map_err(|source| LoggingError::ReadDirectory {
        path: directory.to_owned(),
        source,
    })?;
    let mut owned = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| LoggingError::ReadDirectory {
            path: directory.to_owned(),
            source,
        })?;
        if entry.path() != current && is_owned_debug_filename(&entry.file_name().to_string_lossy())
        {
            owned.push(entry.path());
        }
    }
    owned.sort();
    let remove_count = owned
        .len()
        .saturating_add(1)
        .saturating_sub(RETAINED_DEBUG_LOGS)
        .min(MAX_REMOVALS_PER_START);
    for path in owned.into_iter().take(remove_count) {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => tracing::warn!(
                component = "logging",
                operation = "debug_log_cleanup",
                error_kind = ?source.kind(),
                "could not remove an old owned DEBUG log"
            ),
        }
    }
    Ok(())
}

fn is_owned_debug_filename(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix("debug-")
        .and_then(|value| value.strip_suffix(".log"))
    else {
        return false;
    };
    let Some((timestamp, pid)) = body.rsplit_once('-') else {
        return false;
    };
    !timestamp.is_empty()
        && timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
}

fn install_debug_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info.location();
        tracing::error!(
            component = "application",
            operation = "panic",
            file = location.map(std::panic::Location::file),
            line = location.map(std::panic::Location::line),
            "application panicked; payload omitted from DEBUG log"
        );
    }));
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{create_debug_log, is_owned_debug_filename, openssh_version_token};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "portdeck-logging-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn debug_files_and_directory_are_owner_only_and_unique() {
        let parent = TestDirectory::new();
        let directory = parent.child("state/portdeck");
        let (first_path, _first) = create_debug_log(&directory).unwrap();
        let (second_path, _second) = create_debug_log(&directory).unwrap();

        assert_ne!(first_path, second_path);
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&first_path), 0o600);
        assert_eq!(mode(&second_path), 0o600);
    }

    #[test]
    fn owned_filename_matching_is_exact() {
        assert!(is_owned_debug_filename("debug-123-456.log"));
        assert!(!is_owned_debug_filename("notes.log"));
        assert!(!is_owned_debug_filename("debug-latest.log"));
        assert!(!is_owned_debug_filename("debug-123-456.log.backup"));
        assert!(!is_owned_debug_filename("debug-123-secret-456.log"));
    }

    #[test]
    fn openssh_version_logging_accepts_only_a_bounded_public_token() {
        assert_eq!(
            openssh_version_token("OpenSSH_9.6p1 Ubuntu, OpenSSL 3.0"),
            Some("OpenSSH_9.6p1")
        );
        assert_eq!(openssh_version_token("password=do-not-log"), None);
        assert_eq!(
            openssh_version_token("OpenSSH_9.6p1\nsecret"),
            Some("OpenSSH_9.6p1")
        );
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}
