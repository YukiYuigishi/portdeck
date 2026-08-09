//! Command-line parsing without terminal or logging side effects.

use thiserror::Error;

/// Top-level action selected by command-line arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Start the terminal user interface.
    Tui,
    /// Print the detected OpenSSH version.
    Diagnose,
    /// Print portdeck's version.
    Version,
    /// Print command-line help.
    Help,
}

/// Validated command-line options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Whether file-backed DEBUG logging is enabled.
    pub debug: bool,
    /// Requested top-level action.
    pub mode: Mode,
}

/// Invalid command-line input.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CliError {
    /// An option was not recognized.
    #[error("unknown argument: {0}")]
    UnknownArgument(String),
    /// One option was supplied more than once.
    #[error("argument supplied more than once: {0}")]
    DuplicateArgument(String),
    /// More than one top-level action was requested.
    #[error("command modes cannot be combined: {0} and {1}")]
    ConflictingModes(String, String),
    /// DEBUG logging is meaningful only for executable application modes.
    #[error("--debug can only be used with the TUI or --diagnose")]
    DebugWithInformationalMode,
}

/// Parses arguments after the executable name.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Options, CliError> {
    let mut debug = false;
    let mut selected: Option<(Mode, String)> = None;

    for argument in arguments {
        if argument == "--debug" {
            if debug {
                return Err(CliError::DuplicateArgument(argument));
            }
            debug = true;
            continue;
        }

        let mode = match argument.as_str() {
            "--diagnose" => Mode::Diagnose,
            "-V" | "--version" => Mode::Version,
            "-h" | "--help" => Mode::Help,
            _ => return Err(CliError::UnknownArgument(argument)),
        };
        if let Some((_, previous)) = selected {
            if previous == argument {
                return Err(CliError::DuplicateArgument(argument));
            }
            return Err(CliError::ConflictingModes(previous, argument));
        }
        selected = Some((mode, argument));
    }

    let mode = selected.map_or(Mode::Tui, |(mode, _)| mode);
    if debug && matches!(mode, Mode::Version | Mode::Help) {
        return Err(CliError::DebugWithInformationalMode);
    }
    Ok(Options { debug, mode })
}

#[cfg(test)]
mod tests {
    use super::{CliError, Mode, Options, parse};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn accepts_debug_tui_and_diagnostic_modes() {
        assert_eq!(
            parse(arguments(&["--debug"])).unwrap(),
            Options {
                debug: true,
                mode: Mode::Tui,
            }
        );
        assert_eq!(
            parse(arguments(&["--debug", "--diagnose"])).unwrap(),
            Options {
                debug: true,
                mode: Mode::Diagnose,
            }
        );
        assert_eq!(
            parse(arguments(&["--diagnose", "--debug"])).unwrap(),
            Options {
                debug: true,
                mode: Mode::Diagnose,
            }
        );
    }

    #[test]
    fn rejects_duplicate_and_conflicting_modes() {
        assert!(matches!(
            parse(arguments(&["--debug", "--debug"])),
            Err(CliError::DuplicateArgument(_))
        ));
        assert!(matches!(
            parse(arguments(&["--diagnose", "--version"])),
            Err(CliError::ConflictingModes(_, _))
        ));
        assert_eq!(
            parse(arguments(&["--debug", "--help"])),
            Err(CliError::DebugWithInformationalMode)
        );
    }
}
