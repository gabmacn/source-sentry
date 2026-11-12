//! The commands of `docs/cli.md`.
//!
//! Each command returns its exit code, or a [`Failure`] that `main` prints.

mod apply;
mod attest;
mod check;
mod diff;
mod explain;
mod init;
mod migrate;
mod records;
mod rename;
mod review;
mod track;
mod untrack;

pub(crate) use diff::shell_word;

use chrono::{DateTime, SubsecRound as _, Utc};

use crate::cli::{Cli, Command};
use crate::exit::Failure;
use crate::output::Printer;
use crate::workspace::Workspace;

/// Runs the command `cli` names.
///
/// # Errors
///
/// Returns the failure that ended the command, with its exit code.
pub(crate) fn run(cli: &Cli, printer: Printer) -> Result<u8, Failure> {
	if matches!(&cli.command, Command::Review(_)) {
		review::require_terminal()?;
	}
	let workspace = Workspace::open(cli.repo.as_deref())?;
	match &cli.command {
		Command::Check(args) => check::run(&workspace, args, printer),
		Command::Explain(args) => explain::run(&workspace, args, printer),
		Command::Diff(args) => diff::run(&workspace, args, printer),
		Command::Attest(args) => attest::run(&workspace, args, printer),
		Command::Review(args) => review::run(&workspace, args),
		Command::Seal(args) => records::seal(&workspace, args, printer),
		Command::Retire(args) => records::retire(&workspace, args, printer),
		Command::Track(args) => track::run(&workspace, args, printer),
		Command::Untrack(args) => untrack::run(&workspace, args, printer),
		Command::Rename(args) => rename::run(&workspace, args, printer),
		Command::Init(args) => init::run(&workspace, args, printer),
		Command::Migrate(args) => migrate::run(&workspace, args, printer),
	}
}

/// The current instant, to the second: the evaluation instant of `check`
/// without `--at`, and the time of every review, seal and retirement.
fn now() -> DateTime<Utc> {
	Utc::now().trunc_subsecs(0)
}

/// Who reviews, seals or retires: the name given, or else git's
/// `user.name`.
///
/// # Errors
///
/// Exit 4 when git's configuration cannot be read.
fn person(
	workspace: &Workspace,
	given: Option<&str>,
) -> Result<Option<String>, Failure> {
	match given {
		Some(name) => Ok(Some(name.to_owned())),
		None => workspace
			.repository
			.default_reviewer()
			.map_err(|error| Failure::backend(&error)),
	}
}
