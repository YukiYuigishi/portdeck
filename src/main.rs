use std::env;
use std::process::ExitCode;

use portdeck::application::{AppState, SessionManager};
use portdeck::config::{RuleStore, TargetDiscovery};
use portdeck::error::{AppError, Result};
use portdeck::runtime::RuntimeDirectory;
use portdeck::ssh::OpenSsh;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("portdeck: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    portdeck::logging::init()?;

    let version = env!("CARGO_PKG_VERSION");
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => run_tui(version),
        [argument] if matches!(argument.as_str(), "-V" | "--version") => {
            println!("portdeck {version}");
            Ok(())
        }
        [argument] if matches!(argument.as_str(), "-h" | "--help") => {
            println!(
                "portdeck {version}\n\nSSH connection and local-forward manager\n\nUSAGE:\n    portdeck [--diagnose | --version | --help]"
            );
            Ok(())
        }
        [argument] if argument == "--diagnose" => diagnose_openssh(),
        [argument, ..] => Err(AppError::UnknownArgument(argument.clone())),
    }
}

fn diagnose_openssh() -> Result<()> {
    let output = OpenSsh::default().version()?;
    if !output.success {
        return Err(AppError::OpenSshDiagnostic(
            output
                .diagnostic()
                .unwrap_or("ssh -V returned a non-zero status")
                .to_owned(),
        ));
    }
    println!(
        "{}",
        output
            .diagnostic()
            .unwrap_or("OpenSSH version output was empty")
    );
    Ok(())
}

fn run_tui(version: &str) -> Result<()> {
    tracing::info!(version, "portdeck starting");

    let targets = TargetDiscovery::from_environment()?.discover()?;
    let mut rule_store = RuleStore::from_environment()?;
    let saved_rules = rule_store.load(&targets)?;
    let runtime = RuntimeDirectory::from_environment()?;
    let mut sessions = SessionManager::new(OpenSsh::default(), runtime, targets)?;
    let openssh_version = sessions.probe_openssh()?;
    tracing::info!(openssh_version, "OpenSSH capability probe succeeded");
    let config_failures = sessions.resolve_target_configs();
    if !config_failures.is_empty() {
        tracing::warn!(
            failures = config_failures.len(),
            "some effective SSH configurations could not be resolved"
        );
    }

    let recovery = sessions.recover_previous_runtime()?;
    tracing::info!(
        terminated = recovery.terminated_targets.len(),
        stale_removed = recovery.removed_stale_paths.len(),
        unknown = recovery.unknown_paths.len(),
        failures = recovery.failures.len(),
        "runtime recovery completed"
    );

    let mut app = AppState::with_store(sessions, saved_rules, rule_store)?;
    let tui_result = portdeck::tui::run(&mut app);
    let shutdown_errors = app.sessions_mut().shutdown_all();

    if !shutdown_errors.is_empty() {
        let detail = shutdown_errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        if let Err(error) = tui_result {
            tracing::error!(%error, "TUI failed before shutdown errors were reported");
        }
        return Err(AppError::Shutdown(detail));
    }

    tui_result?;
    Ok(())
}
