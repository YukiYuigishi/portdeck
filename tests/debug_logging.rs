use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const RAW_OPENSSH_OUTPUT: &str = "FAKE_OPENSSH_RAW_PASSWORD=never-log-this";
const FAKE_ENVIRONMENT_SECRET: &str = "environment-secret-must-not-appear";

struct Fixture {
    root: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "portdeck-debug-cli-test-{}-{unique}",
            std::process::id()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let ssh = bin.join("ssh");
        fs::write(
            &ssh,
            format!("#!/bin/sh\nprintf '%s\\n' '{RAW_OPENSSH_OUTPUT}' >&2\n"),
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, bin }
    }

    fn state_home(&self) -> PathBuf {
        self.root.join("state")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn diagnose(&self, state_home: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_portdeck"));
        command
            .args(["--debug", "--diagnose"])
            .env("PATH", &self.bin)
            .env("HOME", self.home())
            .env("PORTDECK_FAKE_PASSWORD", FAKE_ENVIRONMENT_SECRET);
        if let Some(state_home) = state_home {
            command.env("XDG_STATE_HOME", state_home);
        } else {
            command.env_remove("XDG_STATE_HOME");
        }
        command.output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn debug_events_are_file_only_and_raw_diagnostics_are_redacted() {
    let fixture = Fixture::new();
    let state_home = fixture.state_home();
    let output = fixture.diagnose(Some(&state_home));

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        RAW_OPENSSH_OUTPUT
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    let path = debug_path_from_notice(&stderr);
    assert!(path.starts_with(state_home.join("portdeck")));

    let log = fs::read_to_string(&path).unwrap();
    assert!(log.contains("DEBUG"));
    assert!(log.contains("OpenSSH operation started"));
    assert!(log.contains("OpenSSH operation completed"));
    assert!(log.contains("operation_id"));
    assert!(!log.contains(RAW_OPENSSH_OUTPUT));
    assert!(!log.contains(FAKE_ENVIRONMENT_SECRET));
    assert!(!log.contains("PORTDECK_FAKE_PASSWORD"));
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn debug_log_uses_home_fallback_and_unique_per_run_files() {
    let fixture = Fixture::new();
    let first = fixture.diagnose(None);
    let second = fixture.diagnose(None);

    assert!(first.status.success());
    assert!(second.status.success());
    let first_path = debug_path_from_notice(&String::from_utf8(first.stderr).unwrap());
    let second_path = debug_path_from_notice(&String::from_utf8(second.stderr).unwrap());
    assert_ne!(first_path, second_path);
    assert!(first_path.starts_with(fixture.home().join(".local/state/portdeck")));
    assert!(second_path.exists());
}

#[test]
fn cleanup_removes_only_old_owned_names_and_preserves_user_files() {
    let fixture = Fixture::new();
    let directory = fixture.state_home().join("portdeck");
    fs::create_dir_all(&directory).unwrap();
    for index in 0..12 {
        fs::write(directory.join(format!("debug-{index:03}-1.log")), "old").unwrap();
    }
    let user_file = directory.join("debug-latest.log");
    fs::write(&user_file, "mine").unwrap();

    let output = fixture.diagnose(Some(&fixture.state_home()));

    assert!(output.status.success());
    assert_eq!(fs::read_to_string(user_file).unwrap(), "mine");
    let owned_count = fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| is_owned_name(&entry.file_name().to_string_lossy()))
        .count();
    assert_eq!(owned_count, 10);
}

#[test]
fn invalid_state_home_fails_instead_of_silently_disabling_debug() {
    let fixture = Fixture::new();
    let output = fixture.diagnose(Some(Path::new("relative-state")));

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "portdeck: XDG_STATE_HOME must be absolute: relative-state\n"
    );
}

fn debug_path_from_notice(stderr: &str) -> PathBuf {
    let line = stderr.trim();
    PathBuf::from(
        line.strip_prefix("portdeck: DEBUG log: ")
            .unwrap_or_else(|| panic!("unexpected stderr: {stderr:?}")),
    )
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn is_owned_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix("debug-")
        .and_then(|value| value.strip_suffix(".log"))
    else {
        return false;
    };
    let Some((timestamp, pid)) = body.rsplit_once('-') else {
        return false;
    };
    timestamp.bytes().all(|byte| byte.is_ascii_digit())
        && pid.bytes().all(|byte| byte.is_ascii_digit())
}
