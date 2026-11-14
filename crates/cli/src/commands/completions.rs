//! `completions`: the shell completion script, from clap's description of the
//! command line. It needs no repository.

use std::io::{self, Write as _};

use clap::CommandFactory as _;

use crate::cli::{Cli, CompletionsArgs};
use crate::exit::{self, Failure};

/// Prints the completion script for the requested shell.
///
/// # Errors
///
/// Fails when standard output cannot be written.
pub(crate) fn run(args: &CompletionsArgs) -> Result<u8, Failure> {
	let script = script(args.shell);
	let mut stdout = io::stdout().lock();
	stdout
		.write_all(&script)
		.and_then(|()| stdout.flush())
		.map_err(|error| {
			Failure::new(
				exit::OPERATIONAL,
				format!("could not write the completion script: {error}"),
			)
		})?;
	Ok(exit::SUCCESS)
}

/// The script, built in memory: `clap_complete` panics when its writer fails,
/// so it never writes to standard output itself.
fn script(shell: clap_complete::Shell) -> Vec<u8> {
	let mut command = Cli::command();
	let mut script = Vec::new();
	clap_complete::generate(shell, &mut command, "source-sentry", &mut script);
	script
}

#[cfg(test)]
mod tests {
	use clap::ValueEnum as _;

	use super::script;

	/// Every shell gets a script that names the binary and its commands.
	#[test]
	fn every_shell_gets_a_script_naming_the_commands() {
		for shell in clap_complete::Shell::value_variants() {
			let script = String::from_utf8(script(*shell)).unwrap();
			assert!(script.contains("source-sentry"), "{shell}: {script}");
			for command in ["check", "review", "attest", "completions"] {
				assert!(script.contains(command), "{shell} lacks {command}");
			}
		}
	}
}
