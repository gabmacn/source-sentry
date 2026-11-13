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
use source_sentry_core::ports::Snapshot as _;

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
	let mut workspace = Workspace::open(cli.repo.as_deref())?;
	let argument = match &cli.command {
		Command::Diff(args) => Some(&args.document),
		Command::Attest(args) => Some(&args.document),
		Command::Seal(args) | Command::Retire(args) => Some(&args.document),
		Command::Untrack(args) => Some(&args.document),
		Command::Rename(args) => Some(&args.old),
		Command::Track(args) => args.documents.first(),
		_ => None,
	};
	if let Some(crate::cli::DocumentArg::Path(path)) = argument {
		workspace.scope = workspace.repo_path(path)?;
	}
	if let Command::Explain(args) = &cli.command
		&& let Some(path) = explain_path(&workspace, args)?
	{
		workspace.scope = path;
	}
	match &cli.command {
		Command::Check(args) => check::run(&workspace, args, printer),
		Command::Explain(args) => explain::run(&workspace, args, printer),
		Command::Diff(args) => diff::run(&workspace, args, printer),
		Command::Attest(args) => attest::run(&workspace, args, printer),
		Command::Review(args) => review::run(&workspace, args, printer),
		Command::Seal(args) => records::seal(&workspace, args, printer),
		Command::Retire(args) => records::retire(&workspace, args, printer),
		Command::Track(args) => track::run(&workspace, args, printer),
		Command::Untrack(args) => untrack::run(&workspace, args, printer),
		Command::Rename(args) => rename::run(&workspace, args, printer),
		Command::Init(args) => init::run(&workspace, args, printer),
		Command::Migrate(args) => migrate::run(&workspace, args, printer),
	}
}

/// A document selects its project; a series name that also names a directory
/// stays in the starting project, including when inspecting a committed tree.
fn explain_path(
	workspace: &Workspace,
	args: &crate::cli::ExplainArgs,
) -> Result<Option<source_sentry_core::model::RepoPath>, Failure> {
	let Ok(crate::cli::DocumentArg::Path(path)) =
		crate::cli::parse_document(&args.subject)
	else {
		return Ok(None);
	};
	let Ok(path) = workspace.repo_path(&path) else {
		return Ok(None);
	};
	let paths = if let Some(revision) = &args.evaluation.rev {
		workspace
			.repository
			.commit_snapshot(revision)
			.map_err(|error| Failure::backend(&error))?
			.paths()
	} else {
		workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?
			.paths()
	}
	.map_err(|error| Failure::backend(&error))?;
	Ok(
		(!paths
			.iter()
			.any(|candidate| candidate != &path && candidate.starts_with(&path)))
		.then_some(path),
	)
}

/// The current instant, to the second: the evaluation instant of `check`
/// without `--at`, and the time of every review, seal and retirement.
fn now() -> DateTime<Utc> {
	Utc::now().trunc_subsecs(0)
}

/// Who reviews, seals or retires: the name given, or else the one git's
/// configuration remembers as `source-sentry.reviewer`, or else git's
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
