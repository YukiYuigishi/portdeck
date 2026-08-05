use std::process::ExitCode;

use portdeck::error::Result;

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
    tracing::info!(version, "portdeck started");
    println!("portdeck {version}");

    Ok(())
}
