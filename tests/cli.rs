use std::process::Command;

fn portdeck() -> Command {
    Command::new(env!("CARGO_BIN_EXE_portdeck"))
}

#[test]
fn version_reports_the_package_version() {
    let output = portdeck().arg("--version").output().unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("portdeck {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn help_documents_the_supported_cli_modes() {
    let output = portdeck().arg("--help").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("SSH connection and local-forward manager"));
    assert!(stdout.contains("portdeck [--debug] [--diagnose]"));
    assert!(stdout.contains("XDG State directory"));
    assert!(stdout.contains("raw OpenSSH output are omitted"));
    assert!(output.stderr.is_empty());
}

#[test]
fn unknown_arguments_fail_with_a_clear_diagnostic() {
    let output = portdeck().arg("--unsupported").output().unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "portdeck: unknown argument: --unsupported\n"
    );
}
