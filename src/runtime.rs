//! Runtime directory and ControlPath lifecycle management.
//!
//! Resources managed here belong exclusively to portdeck.

use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::domain::TargetId;
use crate::platform::effective_uid;

const APPLICATION_DIRECTORY: &str = "portdeck";
const CONTROL_PREFIX: &str = "cm-";
const CONTROL_HASH_BYTES: usize = 16;
const SAFE_UNIX_SOCKET_PATH_LENGTH: usize = 100;
const OPENSSH_TEMPORARY_SUFFIX_RESERVE: usize = 17;
const SAFE_CONTROL_PATH_LENGTH: usize =
    SAFE_UNIX_SOCKET_PATH_LENGTH - OPENSSH_TEMPORARY_SUFFIX_RESERVE;
const SHORT_TEMPORARY_DIRECTORY: &str = "/tmp";

/// Owner-only directory containing portdeck ControlPath sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDirectory {
    path: PathBuf,
}

impl RuntimeDirectory {
    /// Resolves and prepares the Unix runtime directory.
    pub fn from_environment() -> Result<Self, RuntimeError> {
        let path = resolve_runtime_path(
            env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()),
            &env::temp_dir(),
            effective_uid(),
        )?;

        Self::prepare(path)
    }

    /// Prepares an explicit runtime directory and verifies ownership and mode.
    pub fn prepare(path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(RuntimeError::RelativeRuntimeDirectory(path));
        }

        let uid = effective_uid();
        match fs::symlink_metadata(&path) {
            Ok(metadata) => validate_existing_directory(&path, &metadata, uid)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder
                    .create(&path)
                    .map_err(|source| RuntimeError::Create {
                        path: path.clone(),
                        source,
                    })?;
            }
            Err(source) => {
                return Err(RuntimeError::Inspect {
                    path: path.clone(),
                    source,
                });
            }
        }

        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|source| {
            RuntimeError::Permissions {
                path: path.clone(),
                source,
            }
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| RuntimeError::Inspect {
            path: path.clone(),
            source,
        })?;
        validate_existing_directory(&path, &metadata, uid)?;

        Ok(Self { path })
    }

    /// Directory containing only portdeck-owned runtime entries.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a short stable ControlPath derived from an application target ID.
    pub fn control_path(&self, target_id: &TargetId) -> Result<PathBuf, RuntimeError> {
        let digest = Sha256::digest(target_id.as_str().as_bytes());
        let mut name = String::with_capacity(CONTROL_PREFIX.len() + CONTROL_HASH_BYTES * 2);
        name.push_str(CONTROL_PREFIX);
        for byte in &digest[..CONTROL_HASH_BYTES] {
            write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
        }

        let path = self.path.join(name);
        let length = path.as_os_str().as_bytes().len();
        if length > SAFE_CONTROL_PATH_LENGTH {
            return Err(RuntimeError::ControlPathTooLong {
                path,
                length,
                maximum: SAFE_CONTROL_PATH_LENGTH,
            });
        }
        Ok(path)
    }

    /// Lists entries whose names are in portdeck's ControlPath namespace.
    pub fn owned_control_paths(&self) -> Result<Vec<PathBuf>, RuntimeError> {
        let entries = fs::read_dir(&self.path).map_err(|source| RuntimeError::ReadDirectory {
            path: self.path.clone(),
            source,
        })?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| RuntimeError::ReadDirectory {
                path: self.path.clone(),
                source,
            })?;
            if entry
                .file_name()
                .as_bytes()
                .starts_with(CONTROL_PREFIX.as_bytes())
            {
                paths.push(entry.path());
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Removes a socket only after its ControlMaster was independently found stale.
    pub fn remove_stale_control_path(&self, path: &Path) -> Result<(), RuntimeError> {
        let is_owned = path.parent() == Some(self.path.as_path())
            && path
                .file_name()
                .is_some_and(|name| name.as_bytes().starts_with(CONTROL_PREFIX.as_bytes()));
        if !is_owned {
            return Err(RuntimeError::UnownedControlPath(path.to_owned()));
        }

        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RuntimeError::RemoveStale {
                path: path.to_owned(),
                source,
            }),
        }
    }
}

fn resolve_runtime_path(
    xdg_runtime_directory: Option<OsString>,
    temporary_directory: &Path,
    uid: u32,
) -> Result<PathBuf, RuntimeError> {
    if let Some(base) = xdg_runtime_directory {
        let base = PathBuf::from(base);
        if !base.is_absolute() {
            return Err(RuntimeError::RelativeXdgRuntimeDirectory(base));
        }
        Ok(base.join(APPLICATION_DIRECTORY))
    } else {
        let directory_name = format!("{APPLICATION_DIRECTORY}-{uid}");
        let candidate = temporary_directory.join(&directory_name);
        if control_path_length(&candidate) <= SAFE_CONTROL_PATH_LENGTH {
            Ok(candidate)
        } else {
            Ok(Path::new(SHORT_TEMPORARY_DIRECTORY).join(directory_name))
        }
    }
}

fn control_path_length(runtime_directory: &Path) -> usize {
    let filename = format!("{CONTROL_PREFIX}{}", "0".repeat(CONTROL_HASH_BYTES * 2));
    runtime_directory
        .join(filename)
        .as_os_str()
        .as_bytes()
        .len()
}

/// Runtime directory or ControlPath management failure.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// XDG_RUNTIME_DIR must be absolute when set.
    #[error("XDG_RUNTIME_DIR must be absolute: {0}")]
    RelativeXdgRuntimeDirectory(PathBuf),
    /// An explicitly supplied runtime directory must be absolute.
    #[error("runtime directory must be absolute: {0}")]
    RelativeRuntimeDirectory(PathBuf),
    /// Existing path metadata could not be inspected.
    #[error("failed to inspect runtime path {path}: {source}")]
    Inspect {
        /// Runtime path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Existing runtime path was not a real directory.
    #[error("runtime path is not a directory: {0}")]
    NotDirectory(PathBuf),
    /// Existing runtime directory belongs to another user.
    #[error("runtime directory {path} is owned by uid {actual}, expected uid {expected}")]
    WrongOwner {
        /// Runtime path.
        path: PathBuf,
        /// Current user ID.
        expected: u32,
        /// Directory owner ID.
        actual: u32,
    },
    /// Runtime directory could not be created.
    #[error("failed to create runtime directory {path}: {source}")]
    Create {
        /// Runtime path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Owner-only permissions could not be applied.
    #[error("failed to set runtime directory permissions on {path}: {source}")]
    Permissions {
        /// Runtime path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// A fixed-length hash could not compensate for an unusually long base path.
    #[error("ControlPath is {length} bytes (maximum {maximum}): {path}")]
    ControlPathTooLong {
        /// Generated path.
        path: PathBuf,
        /// Actual byte length.
        length: usize,
        /// Conservative Unix socket path limit.
        maximum: usize,
    },
    /// Runtime directory entries could not be listed.
    #[error("failed to read runtime directory {path}: {source}")]
    ReadDirectory {
        /// Runtime path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// Removal was requested outside portdeck's path namespace.
    #[error("refusing to remove unowned ControlPath: {0}")]
    UnownedControlPath(PathBuf),
    /// A confirmed stale socket could not be removed.
    #[error("failed to remove stale ControlPath {path}: {source}")]
    RemoveStale {
        /// Stale path.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
}

fn validate_existing_directory(
    path: &Path,
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<(), RuntimeError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeError::NotDirectory(path.to_owned()));
    }
    if metadata.uid() != expected_uid {
        return Err(RuntimeError::WrongOwner {
            path: path.to_owned(),
            expected: expected_uid,
            actual: metadata.uid(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::domain::TargetId;
    use crate::platform::effective_uid;

    use super::{
        RuntimeDirectory, RuntimeError, resolve_runtime_path, validate_existing_directory,
    };

    struct TestDirectory(PathBuf);

    static NEXT_TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    impl TestDirectory {
        fn new() -> Self {
            let unique = NEXT_TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/tmp").join(format!("r{:x}{unique:x}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn child(&self, name: &str) -> PathBuf {
            self.0.join(name)
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

    #[test]
    fn creates_owner_only_runtime_directory() {
        let parent = TestDirectory::new();
        let path = parent.child("r");

        let runtime = RuntimeDirectory::prepare(&path).unwrap();

        assert_eq!(runtime.path(), path);
        assert_eq!(fs::metadata(&path).unwrap().uid(), effective_uid());
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn resolves_explicit_xdg_and_uid_scoped_fallback_paths() {
        let parent = TestDirectory::new();
        let xdg = parent.child("xdg");

        assert_eq!(
            resolve_runtime_path(Some(xdg.as_os_str().to_owned()), parent.path(), 42).unwrap(),
            xdg.join("portdeck")
        );
        assert_eq!(
            resolve_runtime_path(None, Path::new("/tmp"), 42).unwrap(),
            Path::new("/tmp/portdeck-42")
        );
        let long_temporary_directory = PathBuf::from(format!("/{}", "x".repeat(80)));
        assert_eq!(
            resolve_runtime_path(None, &long_temporary_directory, 42).unwrap(),
            Path::new("/tmp/portdeck-42")
        );
        assert!(matches!(
            resolve_runtime_path(Some("relative".into()), parent.path(), 42),
            Err(RuntimeError::RelativeXdgRuntimeDirectory(_))
        ));
    }

    #[test]
    fn preserves_an_explicit_xdg_path_for_clear_length_validation() {
        let base = PathBuf::from(format!("/{}", "x".repeat(80)));
        let path = resolve_runtime_path(Some(base.clone().into_os_string()), Path::new("/tmp"), 42)
            .unwrap();
        assert_eq!(path, base.join("portdeck"));

        let runtime = RuntimeDirectory { path };
        assert!(matches!(
            runtime.control_path(&TargetId::new("dev")),
            Err(RuntimeError::ControlPathTooLong { .. })
        ));
    }

    #[test]
    fn rejects_a_runtime_directory_owned_by_another_uid() {
        let parent = TestDirectory::new();
        let metadata = fs::symlink_metadata(parent.path()).unwrap();
        let actual = metadata.uid();
        let expected = actual.wrapping_add(1);

        assert!(matches!(
            validate_existing_directory(parent.path(), &metadata, expected),
            Err(RuntimeError::WrongOwner {
                expected: rejected,
                actual: owner,
                ..
            }) if rejected == expected && owner == actual
        ));
    }

    #[test]
    fn control_paths_are_short_stable_and_do_not_expose_aliases() {
        let parent = TestDirectory::new();
        let runtime = RuntimeDirectory::prepare(parent.child("r")).unwrap();

        let first = runtime
            .control_path(&TargetId::new("very-long-sensitive-target-name"))
            .unwrap();
        let repeated = runtime
            .control_path(&TargetId::new("very-long-sensitive-target-name"))
            .unwrap();
        let second = runtime.control_path(&TargetId::new("other")).unwrap();

        assert_eq!(first, repeated);
        assert_ne!(first, second);
        assert!(!first.to_string_lossy().contains("sensitive"));
        assert_eq!(first.file_name().unwrap().as_bytes().len(), 35);
    }

    #[test]
    fn lists_and_removes_only_namespaced_control_paths() {
        let parent = TestDirectory::new();
        let runtime = RuntimeDirectory::prepare(parent.child("r")).unwrap();
        let owned = runtime.control_path(&TargetId::new("dev")).unwrap();
        let unrelated = runtime.path().join("other-tool");
        fs::write(&owned, "stale").unwrap();
        fs::write(&unrelated, "keep").unwrap();

        let owned_paths = runtime.owned_control_paths().unwrap();
        assert_eq!(owned_paths.len(), 1);
        assert_eq!(owned_paths[0], owned);
        runtime.remove_stale_control_path(&owned).unwrap();

        assert!(!owned.exists());
        assert!(unrelated.exists());
        assert!(matches!(
            runtime.remove_stale_control_path(&unrelated),
            Err(RuntimeError::UnownedControlPath(_))
        ));
    }

    #[test]
    fn refuses_to_adopt_a_symlink_as_runtime_directory() {
        let parent = TestDirectory::new();
        let real = parent.child("real");
        let link = parent.child("link");
        fs::create_dir(&real).unwrap();
        symlink(&real, &link).unwrap();

        assert!(matches!(
            RuntimeDirectory::prepare(link),
            Err(RuntimeError::NotDirectory(_))
        ));
    }

    #[test]
    fn rejects_relative_runtime_paths() {
        assert!(matches!(
            RuntimeDirectory::prepare("relative"),
            Err(RuntimeError::RelativeRuntimeDirectory(_))
        ));
    }

    #[test]
    fn reserves_the_openssh_temporary_suffix_at_the_control_path_boundary() {
        let at_limit = RuntimeDirectory {
            path: PathBuf::from(format!("/{}", "x".repeat(46))),
        };
        let accepted = at_limit.control_path(&TargetId::new("dev")).unwrap();
        assert_eq!(accepted.as_os_str().as_bytes().len(), 83);

        let over_limit = RuntimeDirectory {
            path: PathBuf::from(format!("/{}", "x".repeat(47))),
        };
        assert!(matches!(
            over_limit.control_path(&TargetId::new("dev")),
            Err(RuntimeError::ControlPathTooLong {
                length: 84,
                maximum: 83,
                ..
            })
        ));
    }

    #[test]
    fn tightens_existing_directory_permissions() {
        let parent = TestDirectory::new();
        let path = parent.child("r");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

        RuntimeDirectory::prepare(&path).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn test_directory_is_absolute() {
        let directory = TestDirectory::new();
        assert!(directory.path().is_absolute());
    }
}
