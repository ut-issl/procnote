use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "procnote",
    bin_name = "procnote",
    version,
    about = "Procedure execution tool for hardware testing."
)]
struct Args {
    /// Workspace directory containing procedure subdirectories.
    /// Defaults to the current working directory.
    #[arg(default_value = ".", value_name = "WORKSPACE")]
    workspace: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();

    match procnote_launcher::launch(&args.workspace) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "procnote: {error}");
            ExitCode::FAILURE
        }
    }
}
