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
mod mark;
mod output;
mod workspace;

use std::io::{self, IsTerminal as _};
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
	let matches = command(io::stdout().is_terminal()).try_get_matches()?;
	Cli::from_matches(&matches)
}

/// The command line's definition. On a terminal, the top-level help opens
/// with the mark; redirected help stays plain text for pagers and scripts.
fn command(terminal: bool) -> clap::Command {
	let command = Cli::command();
	if terminal {
		command.before_help(mark::MARK)
	} else {
		command
	}
}

#[cfg(test)]
mod tests {
	use super::{command, mark};

	/// The mark opens the top-level help with its indentation intact, and is
	/// left out when the help is redirected.
	#[test]
	fn the_top_level_help_opens_with_the_mark_on_a_terminal_only() {
		let help = command(true).render_help().to_string();
		assert!(help.starts_with(&format!("{}\n\n", mark::MARK)), "{help}");
		let long = command(true).render_long_help().to_string();
		assert!(long.starts_with(&format!("{}\n\n", mark::MARK)), "{long}");
		let plain = command(false).render_help().to_string();
		assert!(!plain.contains('█'), "{plain}");
	}

	/// A command's own help stays free of the mark.
	#[test]
	fn a_commands_help_does_not_carry_the_mark() {
		let mut root = command(true);
		root.build();
		for name in ["check", "review", "help"] {
			let help = root
				.find_subcommand_mut(name)
				.unwrap()
				.render_long_help()
				.to_string();
			assert!(!help.contains('█'), "{name}: {help}");
		}
	}
}
