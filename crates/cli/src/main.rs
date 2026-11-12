//! The `source-sentry` command.
//!
//! Every command runs the same pipeline: open the repository, take a
//! snapshot (the working tree, or `--rev`), load the configs and discover the
//! inventory, evaluate or plan, render, and exit. The core decides; this
//! crate connects it to the git backend, renders its results, asks for
//! confirmation and sets the exit code.

mod cli;
mod commands;
mod describe;
mod exit;
mod output;
mod workspace;

use std::process::ExitCode;

use clap::CommandFactory as _;

use crate::cli::Cli;
use crate::output::Printer;

fn main() -> ExitCode {
	let cli = match parse() {
		Ok(cli) => cli,
		Err(error) => {
			// Help and the version go to standard output and exit 0; every
			// other parse error is invalid usage, which exits 64 rather than
			// clap's 2, because 2 means a failure.
			let _ = error.print();
			return ExitCode::from(if error.use_stderr() { exit::USAGE } else { 0 });
		}
	};
	output::set_color(cli.color);
	let printer = Printer::new(cli.quiet);
	match commands::run(&cli, printer) {
		Ok(code) => ExitCode::from(code),
		Err(failure) => {
			output::error(&failure.message);
			ExitCode::from(failure.code)
		}
	}
}

/// Parses the command line, without exiting.
fn parse() -> Result<Cli, clap::Error> {
	let matches = Cli::command().try_get_matches()?;
	Cli::from_matches(&matches)
}
