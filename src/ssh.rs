//! System OpenSSH command construction and execution.
//!
//! Commands in this module are passed as argument vectors without a shell.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

/// Validated and normalized OpenSSH `-L` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalForwardSpec {
    bind_address: String,
    local_port: u16,
    remote_host: String,
    remote_port: u16,
}

impl LocalForwardSpec {
    /// Validates local-forward fields.
    pub fn new(
        bind_address: impl Into<String>,
        local_port: u16,
        remote_host: impl Into<String>,
        remote_port: u16,
    ) -> Result<Self, InputError> {
        if local_port == 0 {
            return Err(InputError::ZeroPort {
                field: "local port",
            });
        }
        if remote_port == 0 {
            return Err(InputError::ZeroPort {
                field: "remote port",
            });
        }

        let bind_address = normalize_forward_host(&bind_address.into(), "bind address")?;
        let remote_host = normalize_forward_host(&remote_host.into(), "remote host")?;

        Ok(Self {
            bind_address,
            local_port,
            remote_host,
            remote_port,
        })
    }

    /// Address on the local side where OpenSSH listens.
    pub fn bind_address(&self) -> &str {
        &self.bind_address
    }

    /// Port on the local side where OpenSSH listens.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Host reached from the remote side.
    pub fn remote_host(&self) -> &str {
        &self.remote_host
    }

    /// Port reached from the remote side.
    pub fn remote_port(&self) -> u16 {
        self.remote_port
    }

    /// Returns the exact value passed after `-L` for add and cancel.
    pub fn as_argument(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            format_forward_host(&self.bind_address),
            self.local_port,
            format_forward_host(&self.remote_host),
            self.remote_port
        )
    }

    /// Returns whether the bind explicitly exposes the listener beyond loopback.
    pub fn is_public_bind(&self) -> bool {
        matches!(self.bind_address.as_str(), "0.0.0.0" | "::" | "*")
    }
}

/// A process invocation ready to execute without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshCommand {
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl SshCommand {
    fn new(program: PathBuf, arguments: Vec<OsString>) -> Self {
        Self { program, arguments }
    }

    /// Executable path or name.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Ordered argv values excluding argv[0].
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }
}

/// How child standard streams are connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Capture stdout and stderr for diagnostics.
    Capture,
    /// Give OpenSSH terminal input while retaining stderr for later diagnostics.
    Interactive,
}

/// Structured result from a successfully spawned OpenSSH process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshOutput {
    /// Whether OpenSSH returned a successful exit status.
    pub success: bool,
    /// Platform exit code, or `None` when terminated by a signal.
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

impl SshOutput {
    /// Returns trimmed stderr when available, otherwise trimmed stdout.
    pub fn diagnostic(&self) -> Option<&str> {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return Some(stderr);
        }

        let stdout = self.stdout.trim();
        (!stdout.is_empty()).then_some(stdout)
    }
}

/// Abstraction around process spawning for deterministic adapter tests.
pub trait CommandExecutor {
    /// Executes one argv-based OpenSSH command.
    fn execute(&self, command: &SshCommand, mode: ExecutionMode) -> io::Result<SshOutput>;
}

/// Executes commands using [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCommandExecutor;

static NEXT_STDERR_FILE_ID: AtomicU64 = AtomicU64::new(0);
const STDERR_FILE_CREATE_ATTEMPTS: usize = 128;
const STDERR_READ_BUFFER_SIZE: usize = 8 * 1024;

fn private_unlinked_stderr_file() -> io::Result<File> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for _ in 0..STDERR_FILE_CREATE_ATTEMPTS {
        let sequence = NEXT_STDERR_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            ".portdeck-stderr-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                if let Err(source) = fs::remove_file(&path) {
                    drop(file);
                    let _ = fs::remove_file(path);
                    return Err(source);
                }
                return Ok(file);
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(source),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file for OpenSSH stderr",
    ))
}

fn captured_stderr(file: &File) -> io::Result<Vec<u8>> {
    let length = file.metadata()?.len();
    let capacity = usize::try_from(length)
        .map_err(|_| io::Error::other("OpenSSH stderr is too large to capture"))?;
    let mut contents = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; STDERR_READ_BUFFER_SIZE];
    let mut offset = 0_u64;

    while offset < length {
        let remaining = length - offset;
        let read_length = usize::try_from(remaining.min(STDERR_READ_BUFFER_SIZE as u64))
            .expect("stderr read length always fits usize");
        match file.read_at(&mut buffer[..read_length], offset) {
            Ok(0) => break,
            Ok(count) => {
                contents.extend_from_slice(&buffer[..count]);
                offset += u64::try_from(count).expect("stderr read count always fits u64");
            }
            Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
            Err(source) => return Err(source),
        }
    }

    Ok(contents)
}

impl CommandExecutor for SystemCommandExecutor {
    fn execute(&self, command: &SshCommand, mode: ExecutionMode) -> io::Result<SshOutput> {
        let mut process = Command::new(command.program());
        process.args(command.arguments());

        match mode {
            ExecutionMode::Capture => {
                let output = process.output()?;
                Ok(SshOutput {
                    success: output.status.success(),
                    exit_code: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            }
            ExecutionMode::Interactive => {
                let stderr_file = private_unlinked_stderr_file()?;
                let status = process
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::from(stderr_file.try_clone()?))
                    .status()?;
                let stderr = captured_stderr(&stderr_file)?;
                Ok(SshOutput {
                    success: status.success(),
                    exit_code: status.code(),
                    stdout: String::new(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                })
            }
        }
    }
}

/// OpenSSH adapter parameterized by its process executor.
#[derive(Debug, Clone)]
pub struct OpenSsh<E = SystemCommandExecutor> {
    executable: PathBuf,
    executor: E,
}

/// Operations required by the application layer from an SSH backend.
pub trait SshClient {
    /// Detects the executable and reports its version text.
    fn version(&self) -> Result<SshOutput, SshError>;
    /// Resolves effective configuration for one concrete alias.
    fn resolve_config(&self, host_alias: &str) -> Result<SshOutput, SshError>;
    /// Starts a dedicated ControlMaster.
    fn connect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError>;
    /// Checks a dedicated ControlMaster.
    fn check(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError>;
    /// Adds one local forward.
    fn add_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError>;
    /// Cancels one exact local forward.
    fn cancel_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError>;
    /// Stops a dedicated ControlMaster.
    fn disconnect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError>;
}

impl Default for OpenSsh<SystemCommandExecutor> {
    fn default() -> Self {
        Self::new("ssh")
    }
}

impl OpenSsh<SystemCommandExecutor> {
    /// Creates an adapter backed by the system process executor.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            executor: SystemCommandExecutor,
        }
    }
}

impl<E: CommandExecutor> OpenSsh<E> {
    /// Creates an adapter with a replaceable executor.
    pub fn with_executor(executable: impl Into<PathBuf>, executor: E) -> Self {
        Self {
            executable: executable.into(),
            executor,
        }
    }

    /// Runs `ssh -V` to detect the executable and version text.
    pub fn version(&self) -> Result<SshOutput, SshError> {
        self.capture(vec![OsString::from("-V")])
    }

    /// Runs `ssh -G <alias>` so OpenSSH resolves effective configuration.
    pub fn resolve_config(&self, host_alias: &str) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.capture(vec![OsString::from("-G"), OsString::from(host_alias)])
    }

    /// Starts a dedicated ControlMaster while inheriting the current terminal.
    pub fn connect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.execute(
            vec![
                OsString::from("-M"),
                OsString::from("-N"),
                OsString::from("-f"),
                OsString::from("-S"),
                control_path.as_os_str().to_owned(),
                OsString::from("-o"),
                OsString::from("ClearAllForwardings=yes"),
                OsString::from(host_alias),
            ],
            ExecutionMode::Interactive,
        )
    }

    /// Checks a dedicated ControlMaster with `-O check`.
    pub fn check(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.capture(control_operation_arguments(
            host_alias,
            control_path,
            "check",
        ))
    }

    /// Adds one local forward to a dedicated ControlMaster.
    pub fn add_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.capture(forward_operation_arguments(
            host_alias,
            control_path,
            "forward",
            forward,
        ))
    }

    /// Cancels the exact normalized local forward previously added.
    pub fn cancel_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.capture(forward_operation_arguments(
            host_alias,
            control_path,
            "cancel",
            forward,
        ))
    }

    /// Stops a dedicated ControlMaster with `-O exit`.
    pub fn disconnect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        validate_host_alias(host_alias)?;
        self.capture(control_operation_arguments(
            host_alias,
            control_path,
            "exit",
        ))
    }

    fn capture(&self, arguments: Vec<OsString>) -> Result<SshOutput, SshError> {
        self.execute(arguments, ExecutionMode::Capture)
    }

    fn execute(
        &self,
        arguments: Vec<OsString>,
        mode: ExecutionMode,
    ) -> Result<SshOutput, SshError> {
        let command = SshCommand::new(self.executable.clone(), arguments);
        self.executor
            .execute(&command, mode)
            .map_err(|source| SshError::Execute {
                executable: self.executable.clone(),
                source,
            })
    }
}

impl<E: CommandExecutor> SshClient for OpenSsh<E> {
    fn version(&self) -> Result<SshOutput, SshError> {
        OpenSsh::version(self)
    }

    fn resolve_config(&self, host_alias: &str) -> Result<SshOutput, SshError> {
        OpenSsh::resolve_config(self, host_alias)
    }

    fn connect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        OpenSsh::connect(self, host_alias, control_path)
    }

    fn check(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        OpenSsh::check(self, host_alias, control_path)
    }

    fn add_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError> {
        OpenSsh::add_local_forward(self, host_alias, control_path, forward)
    }

    fn cancel_local_forward(
        &self,
        host_alias: &str,
        control_path: &Path,
        forward: &LocalForwardSpec,
    ) -> Result<SshOutput, SshError> {
        OpenSsh::cancel_local_forward(self, host_alias, control_path, forward)
    }

    fn disconnect(&self, host_alias: &str, control_path: &Path) -> Result<SshOutput, SshError> {
        OpenSsh::disconnect(self, host_alias, control_path)
    }
}

/// Invalid user or SSH configuration input.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InputError {
    /// A required value was empty.
    #[error("{field} must not be empty")]
    Empty {
        /// Field being validated.
        field: &'static str,
    },
    /// A value contained a character unsafe for OpenSSH argv/config usage.
    #[error("{field} contains an unsupported character")]
    UnsupportedCharacter {
        /// Field being validated.
        field: &'static str,
    },
    /// A port used the reserved value zero.
    #[error("{field} must be between 1 and 65535")]
    ZeroPort {
        /// Field being validated.
        field: &'static str,
    },
    /// Square brackets were not a valid IPv6-style pair.
    #[error("{field} has mismatched brackets")]
    MismatchedBrackets {
        /// Field being validated.
        field: &'static str,
    },
    /// Host alias would be parsed by OpenSSH as an option.
    #[error("host alias must not start with '-'")]
    OptionLikeHostAlias,
    /// Host alias is a pattern rather than a concrete target.
    #[error("host alias must not contain wildcard or negation characters")]
    PatternHostAlias,
}

/// Failure before or while spawning the OpenSSH process.
#[derive(Debug, Error)]
pub enum SshError {
    /// User or discovered input was rejected before execution.
    #[error("invalid OpenSSH input: {0}")]
    InvalidInput(#[from] InputError),
    /// The executable could not be launched or waited on.
    #[error("failed to execute {executable}: {source}")]
    Execute {
        /// Configured executable path.
        executable: PathBuf,
        /// Operating-system process error.
        #[source]
        source: io::Error,
    },
}

/// Validates a concrete alias before passing it to OpenSSH.
pub fn validate_host_alias(host_alias: &str) -> Result<(), InputError> {
    if host_alias.is_empty() {
        return Err(InputError::Empty {
            field: "host alias",
        });
    }
    if host_alias.starts_with('-') {
        return Err(InputError::OptionLikeHostAlias);
    }
    if host_alias.contains(['*', '?', '!']) {
        return Err(InputError::PatternHostAlias);
    }
    if host_alias.chars().any(char::is_whitespace) || host_alias.contains('\0') {
        return Err(InputError::UnsupportedCharacter {
            field: "host alias",
        });
    }
    Ok(())
}

fn normalize_forward_host(value: &str, field: &'static str) -> Result<String, InputError> {
    if value.is_empty() {
        return Err(InputError::Empty { field });
    }
    if value.contains(['\0', '\n', '\r']) {
        return Err(InputError::UnsupportedCharacter { field });
    }

    let starts_with_bracket = value.starts_with('[');
    let ends_with_bracket = value.ends_with(']');
    if starts_with_bracket != ends_with_bracket {
        return Err(InputError::MismatchedBrackets { field });
    }

    if starts_with_bracket {
        let inner = &value[1..value.len() - 1];
        if inner.is_empty() || inner.contains(['[', ']']) {
            return Err(InputError::MismatchedBrackets { field });
        }
        return Ok(inner.to_owned());
    }
    if value.contains(['[', ']']) {
        return Err(InputError::MismatchedBrackets { field });
    }

    Ok(value.to_owned())
}

fn format_forward_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn control_operation_arguments(
    host_alias: &str,
    control_path: &Path,
    operation: &str,
) -> Vec<OsString> {
    vec![
        OsString::from("-S"),
        control_path.as_os_str().to_owned(),
        OsString::from("-O"),
        OsString::from(operation),
        OsString::from(host_alias),
    ]
}

fn forward_operation_arguments(
    host_alias: &str,
    control_path: &Path,
    operation: &str,
    forward: &LocalForwardSpec,
) -> Vec<OsString> {
    vec![
        OsString::from("-S"),
        control_path.as_os_str().to_owned(),
        OsString::from("-o"),
        OsString::from("ClearAllForwardings=no"),
        OsString::from("-O"),
        OsString::from(operation),
        OsString::from("-L"),
        OsString::from(forward.as_argument()),
        OsString::from(host_alias),
    ]
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::io;
    use std::path::Path;

    use super::{
        CommandExecutor, ExecutionMode, InputError, LocalForwardSpec, OpenSsh, SshCommand,
        SshOutput, validate_host_alias,
    };

    #[derive(Debug, Default)]
    struct RecordingExecutor {
        calls: RefCell<Vec<(SshCommand, ExecutionMode)>>,
    }

    impl CommandExecutor for RecordingExecutor {
        fn execute(&self, command: &SshCommand, mode: ExecutionMode) -> io::Result<SshOutput> {
            self.calls.borrow_mut().push((command.clone(), mode));
            Ok(SshOutput {
                success: true,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn string_arguments(command: &SshCommand) -> Vec<String> {
        command
            .arguments()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn builds_connect_argv_and_uses_interactive_terminal() {
        let executor = RecordingExecutor::default();
        let ssh = OpenSsh::with_executor("/usr/bin/ssh", executor);

        ssh.connect("dev", Path::new("/run/user/1000/portdeck/cm"))
            .unwrap();

        let calls = ssh.executor.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, ExecutionMode::Interactive);
        assert_eq!(
            string_arguments(&calls[0].0),
            [
                "-M",
                "-N",
                "-f",
                "-S",
                "/run/user/1000/portdeck/cm",
                "-o",
                "ClearAllForwardings=yes",
                "dev",
            ]
        );
    }

    #[test]
    fn builds_forward_and_cancel_with_the_same_normalized_spec() {
        let executor = RecordingExecutor::default();
        let ssh = OpenSsh::with_executor("ssh", executor);
        let forward = LocalForwardSpec::new("::1", 8080, "2001:db8::10", 3000).unwrap();

        ssh.add_local_forward("dev", Path::new("control"), &forward)
            .unwrap();
        ssh.cancel_local_forward("dev", Path::new("control"), &forward)
            .unwrap();

        let calls = ssh.executor.calls.borrow();
        assert_eq!(
            string_arguments(&calls[0].0),
            [
                "-S",
                "control",
                "-o",
                "ClearAllForwardings=no",
                "-O",
                "forward",
                "-L",
                "[::1]:8080:[2001:db8::10]:3000",
                "dev",
            ]
        );
        assert_eq!(
            string_arguments(&calls[1].0),
            [
                "-S",
                "control",
                "-o",
                "ClearAllForwardings=no",
                "-O",
                "cancel",
                "-L",
                "[::1]:8080:[2001:db8::10]:3000",
                "dev",
            ]
        );
    }

    #[test]
    fn normalizes_ipv4_hostname_and_ipv6_forward_values() {
        let ipv4 = LocalForwardSpec::new("127.0.0.1", 5432, "db.internal", 5432).unwrap();
        let ipv6 = LocalForwardSpec::new("[::1]", 8443, "[2001:db8::1]", 443).unwrap();

        assert_eq!(ipv4.as_argument(), "127.0.0.1:5432:db.internal:5432");
        assert_eq!(ipv6.as_argument(), "[::1]:8443:[2001:db8::1]:443");
    }

    #[test]
    fn identifies_explicit_public_binds() {
        for address in ["0.0.0.0", "::", "*"] {
            let forward = LocalForwardSpec::new(address, 8080, "127.0.0.1", 80).unwrap();
            assert!(forward.is_public_bind());
        }

        let loopback = LocalForwardSpec::new("127.0.0.1", 8080, "127.0.0.1", 80).unwrap();
        assert!(!loopback.is_public_bind());
    }

    #[test]
    fn rejects_zero_ports_and_malformed_brackets() {
        assert_eq!(
            LocalForwardSpec::new("127.0.0.1", 0, "db", 5432).unwrap_err(),
            InputError::ZeroPort {
                field: "local port"
            }
        );
        assert_eq!(
            LocalForwardSpec::new("[::1", 8080, "db", 5432).unwrap_err(),
            InputError::MismatchedBrackets {
                field: "bind address"
            }
        );
    }

    #[test]
    fn rejects_option_like_patterns_and_control_characters_in_aliases() {
        for alias in [
            "-oProxyCommand=bad",
            "host*",
            "host?",
            "!host",
            "host\nother",
        ] {
            assert!(validate_host_alias(alias).is_err(), "accepted {alias:?}");
        }
        assert!(validate_host_alias("dev-server").is_ok());
    }

    #[test]
    fn keeps_host_alias_as_one_argument() {
        let executor = RecordingExecutor::default();
        let ssh = OpenSsh::with_executor("ssh", executor);

        ssh.resolve_config("dev;touch-pwned").unwrap();

        let calls = ssh.executor.calls.borrow();
        assert_eq!(
            calls[0].0.arguments(),
            [OsString::from("-G"), OsString::from("dev;touch-pwned")]
        );
    }
}
