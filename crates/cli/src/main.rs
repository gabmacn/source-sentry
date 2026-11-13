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
	// On a terminal the top-level help opens with the mark, shaded where the
	// terminal shows 24-bit colour; redirected help stays plain text for
	// pagers and scripts.
	let mark = io::stdout()
		.is_terminal()
		.then(|| mark::dressed(!colour_refused() && output::full_colour()));
	let matches = command(mark.as_deref()).try_get_matches()?;
	Cli::from_matches(&matches)
}

/// Whether the command line turns colour off. Help is printed before any
/// option takes effect, so the mark's colour looks for this one itself.
fn colour_refused() -> bool {
	let arguments: Vec<_> = std::env::args_os().collect();
	arguments.iter().any(|argument| argument == "--color=never")
		|| arguments
			.windows(2)
			.any(|pair| pair[0] == "--color" && pair[1] == "never")
}

/// The command line's definition, with the mark that opens its top-level
/// help, if any.
fn command(mark: Option<&str>) -> clap::Command {
	let command = Cli::command();
	match mark {
		Some(mark) => command.before_help(mark.to_owned()),
		None => command,
	}
}

#[cfg(test)]
mod tests {
	use super::{command, mark};

	/// The mark opens the top-level help with its indentation intact, and is
	/// left out when the help is redirected.
	#[test]
	fn the_top_level_help_opens_with_the_mark_on_a_terminal_only() {
		let help = command(Some(mark::MARK)).render_help().to_string();
		assert!(help.starts_with(&format!("{}\n\n", mark::MARK)), "{help}");
		let long = command(Some(mark::MARK)).render_long_help().to_string();
		assert!(long.starts_with(&format!("{}\n\n", mark::MARK)), "{long}");
		let plain = command(None).render_help().to_string();
		assert!(!plain.contains('█'), "{plain}");
	}

	/// A shaded mark keeps its colours in help that is printed with colour,
	/// and is the plain mark, line for line, in help printed without.
	#[test]
	fn the_shaded_mark_survives_the_help_with_and_without_colour() {
		let shaded = mark::dressed(true);
		let help = command(Some(&shaded)).render_help();
		let coloured = help.ansi().to_string();
		assert!(
			coloured.starts_with(&format!("{shaded}\n\n")),
			"{coloured:?}"
		);
		let plain = help.to_string();
		assert!(plain.starts_with(&format!("{}\n\n", mark::MARK)), "{plain}");
	}

	/// A command's own help stays free of the mark.
	#[test]
	fn a_commands_help_does_not_carry_the_mark() {
		let mut root = command(Some(mark::MARK));
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
