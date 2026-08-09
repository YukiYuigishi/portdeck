use std::env;
use std::path::Path;
use std::process::ExitCode;

use portdeck::application::{AppState, SessionManager};
use portdeck::cli::{Mode, Options};
use portdeck::config::{RuleStore, TargetDiscovery};
use portdeck::error::{AppError, Result};
use portdeck::runtime::RuntimeDirectory;
use portdeck::ssh::OpenSsh;
use portdeck::tui::StartupNotice;

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
    let version = env!("CARGO_PKG_VERSION");
    let options = portdeck::cli::parse(env::args().skip(1))?;
    match options.mode {
        Mode::Version => {
            println!("portdeck {version}");
            Ok(())
        }
        Mode::Help => {
            println!(
                "portdeck {version}\n\nSSH connection and local-forward manager\n\nUSAGE:\n    portdeck [--debug] [--diagnose]\n    portdeck [--version | --help]\n\nOPTIONS:\n    --debug       Write private DEBUG logs under the XDG State directory\n                  (credentials, raw key input, and raw OpenSSH output are omitted)\n    --diagnose    Print the detected OpenSSH version\n    -V, --version Print portdeck's version\n    -h, --help    Print help"
            );
            Ok(())
        }
        Mode::Diagnose => {
            let logging = initialize_logging(options)?;
            diagnose_openssh(logging.debug_path())
        }
        Mode::Tui => {
            let logging = initialize_logging(options)?;
            run_tui(version, logging.debug_path())
        }
    }
}

fn initialize_logging(options: Options) -> Result<portdeck::logging::LoggingHandle> {
    let logging = portdeck::logging::init(options.debug)?;
    if let Some(path) = logging.debug_path() {
        eprintln!("portdeck: DEBUG log: {}", path.display());
    }
    Ok(logging)
}

fn diagnose_openssh(debug_path: Option<&Path>) -> Result<()> {
    tracing::debug!(
        component = "application",
        operation = "diagnose_openssh",
        debug_log = debug_path.map(Path::display).map(|path| path.to_string()),
        "OpenSSH diagnosis started"
    );
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
    tracing::debug!(
        component = "application",
        operation = "diagnose_openssh",
        success = true,
        "OpenSSH diagnosis completed"
    );
    Ok(())
}

fn run_tui(version: &str, debug_path: Option<&Path>) -> Result<()> {
    tracing::info!(
        component = "application",
        operation = "startup",
        operation_id = portdeck::logging::next_operation_id(),
        version,
        "portdeck starting"
    );

    let discovery_operation_id = portdeck::logging::next_operation_id();
    tracing::debug!(
        component = "config",
        operation = "target_discovery",
        operation_id = discovery_operation_id,
        "SSH target discovery started"
    );
    let targets = TargetDiscovery::from_environment()?.discover()?;
    tracing::debug!(
        component = "config",
        operation = "target_discovery",
        operation_id = discovery_operation_id,
        targets = targets.len(),
        "SSH target discovery completed"
    );
    let mut rule_store = RuleStore::from_environment()?;
    let saved_rules = rule_store.load(&targets)?;
    let runtime = RuntimeDirectory::from_environment()?;
    let mut sessions = SessionManager::new(OpenSsh::default(), runtime, targets)?;
    let openssh_version = sessions.probe_openssh()?;
    let openssh_version =
        portdeck::logging::openssh_version_token(&openssh_version).unwrap_or("unrecognized");
    tracing::info!(
        component = "ssh",
        operation = "version",
        openssh_version,
        "OpenSSH capability probe succeeded"
    );
    let resolution_operation_id = portdeck::logging::next_operation_id();
    tracing::debug!(
        component = "config",
        operation = "effective_config_resolution",
        operation_id = resolution_operation_id,
        targets = sessions.entries().len(),
        "effective SSH configuration resolution started"
    );
    let config_failures = sessions.resolve_target_configs();
    tracing::debug!(
        component = "config",
        operation = "effective_config_resolution",
        operation_id = resolution_operation_id,
        targets = sessions.entries().len(),
        failures = config_failures.len(),
        success = config_failures.is_empty(),
        "effective SSH configuration resolution completed"
    );
    if !config_failures.is_empty() {
        tracing::warn!(
            failures = config_failures.len(),
            "some effective SSH configurations could not be resolved"
        );
    }

    let recovery_operation_id = portdeck::logging::next_operation_id();
    tracing::debug!(
        component = "runtime",
        operation = "recovery",
        operation_id = recovery_operation_id,
        "runtime recovery started"
    );
    let recovery = sessions.recover_previous_runtime()?;
    tracing::info!(
        component = "runtime",
        operation = "recovery",
        operation_id = recovery_operation_id,
        terminated = recovery.terminated_targets.len(),
        stale_removed = recovery.removed_stale_paths.len(),
        unknown = recovery.unknown_paths.len(),
        failures = recovery.failures.len(),
        "runtime recovery completed"
    );

    let mut startup_details = config_failures
        .iter()
        .chain(recovery.failures.iter())
        .map(|failure| {
            failure
                .detail
                .as_ref()
                .map(|detail| format!("{}: {detail}", failure.summary))
                .unwrap_or_else(|| failure.summary.clone())
        })
        .collect::<Vec<_>>();
    if let Some(path) = debug_path {
        startup_details.push(format!("DEBUG log: {}", path.display()));
    }
    startup_details.extend(
        recovery
            .unknown_paths
            .iter()
            .map(|path| format!("未確認のruntime entryを保持しました: {}", path.display())),
    );
    let recovered_count = recovery.terminated_targets.len() + recovery.removed_stale_paths.len();
    let diagnostic_count = config_failures.len() + recovery.failures.len();
    let startup_notice = if diagnostic_count > 0 || !recovery.unknown_paths.is_empty() {
        Some(StartupNotice {
            status: format!(
                "起動診断に{}件の注意があります（e: 詳細）",
                diagnostic_count + recovery.unknown_paths.len()
            ),
            error_detail: Some(startup_details.join("\n")),
        })
    } else if recovered_count > 0 {
        Some(StartupNotice {
            status: debug_path.map_or_else(
                || format!("前回のruntime entryを{recovered_count}件回収しました"),
                |path| {
                    format!(
                        "前回のruntime entryを{recovered_count}件回収 / DEBUG log: {}",
                        path.display()
                    )
                },
            ),
            error_detail: debug_path.map(|path| format!("DEBUG log: {}", path.display())),
        })
    } else {
        debug_path.map(|path| StartupNotice {
            status: format!("DEBUG log: {}", path.display()),
            error_detail: Some(format!("DEBUG log: {}", path.display())),
        })
    };

    let mut app = AppState::with_store(sessions, saved_rules, rule_store)?;
    let tui_result = portdeck::tui::run(&mut app, startup_notice);
    let shutdown_operation_id = portdeck::logging::next_operation_id();
    tracing::debug!(
        component = "application",
        operation = "shutdown",
        operation_id = shutdown_operation_id,
        "owned session shutdown started"
    );
    let shutdown_errors = app.sessions_mut().shutdown_all();
    tracing::debug!(
        component = "application",
        operation = "shutdown",
        operation_id = shutdown_operation_id,
        failures = shutdown_errors.len(),
        success = shutdown_errors.is_empty(),
        "owned session shutdown completed"
    );

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
