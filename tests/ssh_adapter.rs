#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use portdeck::ssh::OpenSsh;

struct FakeSsh {
    directory: PathBuf,
    executable: PathBuf,
    arguments_log: PathBuf,
}

impl FakeSsh {
    fn new(exit_code: u8) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("portdeck-fake-ssh-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();

        let executable = directory.join("ssh");
        let arguments_log = directory.join("arguments");
        let script = format!(
            "#!/bin/sh\n: > '{log}'\nfor argument in \"$@\"; do\n  printf '%s\\n' \"$argument\" >> '{log}'\ndone\nprintf 'fake stdout\\n'\nprintf 'fake stderr\\n' >&2\nexit {exit_code}\n",
            log = arguments_log.display()
        );
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();

        Self {
            directory,
            executable,
            arguments_log,
        }
    }

    fn arguments(&self) -> Vec<String> {
        fs::read_to_string(&self.arguments_log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for FakeSsh {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn captures_fake_ssh_output_status_and_individual_arguments() {
    let fake = FakeSsh::new(23);
    let ssh = OpenSsh::new(&fake.executable);

    let output = ssh
        .check("dev;still-one-argument", Path::new("control path"))
        .unwrap();

    assert!(!output.success);
    assert_eq!(output.exit_code, Some(23));
    assert_eq!(output.stdout, "fake stdout\n");
    assert_eq!(output.stderr, "fake stderr\n");
    assert_eq!(
        fake.arguments(),
        [
            "-S",
            "control path",
            "-O",
            "check",
            "dev;still-one-argument"
        ]
    );
}
