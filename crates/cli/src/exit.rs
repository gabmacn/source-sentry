//! Exit codes, and the failures that end a command.
//!
//! `check` exits with
//! [`policy::ExitCode`](source_sentry_core::policy::ExitCode). The other
//! commands use the codes below, which keep the same meanings where they
//! overlap: 3 for configuration and integrity problems,
//! 4 when the repository could not be read or written, and 64 for invalid
//! usage.

use std::error::Error;
use std::fmt::Write as _;

use source_sentry_core::claims::Inventory;
use source_sentry_core::config::LoadError;
use source_sentry_core::mutate::MutateError;
use source_sentry_core::policy::USAGE_EXIT_CODE;
use source_sentry_core::ports::BackendError;
use source_sentry_git::ApplyError;

use crate::workspace::Workspace;

mod render;

/// The command did what was asked: it applied its plan, found nothing to
/// change, printed a dry run, or printed what was asked for.
pub(crate) const SUCCESS: u8 = 0;
/// A command that changes files was not confirmed: the user declined, or
/// there was no terminal to ask on and no `--yes`.
pub(crate) const NOT_CONFIRMED: u8 = 1;
/// The command refused because the request does not fit the repository,
/// such as an untracked document or a snapshot that differs from the
/// approved expectations.
pub(crate) const REFUSED: u8 = 2;
/// A configuration or integrity error, as for `check`.
pub(crate) const CONFIGURATION: u8 = 3;
/// The repository could not be read or written: the command could not
/// complete, as for `check`.
pub(crate) const OPERATIONAL: u8 = 4;
/// Invalid command-line usage.
pub(crate) const USAGE: u8 = USAGE_EXIT_CODE;

/// Why a command stopped, with the exit code it ends with.
#[derive(Debug)]
pub(crate) struct Failure {
	/// The exit code.
	pub(crate) code: u8,
	/// What happened and what to do, for standard error.
	pub(crate) message: String,
}

impl Failure {
	/// A failure with `code`.
	pub(crate) fn new(code: u8, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
		}
	}

	/// Invalid usage, found after parsing: exit 64.
	pub(crate) fn usage(message: impl Into<String>) -> Self {
		Self::new(USAGE, message)
	}

	/// A backend failure: exit 4.
	pub(crate) fn backend(error: &BackendError) -> Self {
		Self::new(OPERATIONAL, chain(error))
	}

	/// Configs that failed to load: exit 3 when they are invalid, 4 when the
	/// snapshot could not be read.
	pub(crate) fn load(workspace: &Workspace, error: &LoadError) -> Self {
		match error {
			LoadError::Backend(error) => Self::backend(error),
			LoadError::Invalid(errors) => {
				let mut message = String::from("the configs are invalid:");
				for error in errors {
					let _ = write!(message, "\n  - {}", render::config(workspace, error));
				}
				Self::new(CONFIGURATION, message)
			}
		}
	}

	/// A planner's refusal, with the code its cause calls for.
	pub(crate) fn mutate(workspace: &Workspace, error: &MutateError) -> Self {
		Self::new(
			refusal_code(error),
			render::mutation(workspace, None, error),
		)
	}

	/// A planner's refusal, like [`Failure::mutate`], from a planner that
	/// read `inventory`, so that a suggested command can name the lockfile
	/// that would hold the document.
	pub(crate) fn mutate_in(
		workspace: &Workspace,
		inventory: &Inventory,
		error: &MutateError,
	) -> Self {
		Self::new(
			refusal_code(error),
			render::mutation(workspace, Some(inventory), error),
		)
	}

	/// A write plan that could not be applied: exit 4.
	///
	/// A plan that creates a file which already exists usually means a file
	/// that an earlier command created and git does not track yet: snapshots
	/// list git's index, so the hint says to add it.
	pub(crate) fn apply(workspace: &Workspace, error: &ApplyError) -> Self {
		let mut message = match error {
			// The advice names a command, which is written to run where the
			// user started.
			ApplyError::Unverifiable { path, reason } => format!(
				"cannot verify {}: {}",
				workspace.display_path(path),
				render::canonical_problem(workspace, path, reason)
			),
			_ => chain(error),
		};
		if !matches!(error, ApplyError::RollbackFailed { .. }) {
			message.push_str("; nothing was changed");
		}
		if let ApplyError::Stale {
			path,
			expected: None,
			actual: Some(_),
		} = error
		{
			let add = workspace.git_command(&["add"], &[path]);
			let path = workspace.display_path(path);
			let _ = write!(
				message,
				". A file that git does not track already exists at {path}; move \
				 it aside, or add it with `{add}`, and run the command again"
			);
		}
		Self::new(OPERATIONAL, message)
	}
}

/// The exit code of a planner's refusal.
///
/// - 3 when the cause is a configuration or integrity error: invalid
///   configs, lockfiles or frontmatter, a double claim, errors that
///   discovery reports about the document, a change that would introduce
///   errors, or a lockfile schema that cannot be read or migrated.
/// - 4 when the repository could not be read, a file has no canonical oid
///   (an operational error for `check` too), or the plan
///   would not read back, which is a bug.
/// - 2 for every other refusal, which concerns the request itself.
fn refusal_code(error: &MutateError) -> u8 {
	match error {
		MutateError::Backend(_)
		| MutateError::NotCanonical { .. }
		| MutateError::RoundTrip { .. } => OPERATIONAL,
		MutateError::DoubleClaim { .. }
		| MutateError::DocumentHasErrors { .. }
		| MutateError::InvalidLockfile { .. }
		| MutateError::InvalidFrontmatter { .. }
		| MutateError::WouldIntroduce { .. }
		| MutateError::InvalidConfigs { .. }
		| MutateError::UnreadableSchema { .. }
		| MutateError::UnknownSchema { .. } => CONFIGURATION,
		_ => REFUSED,
	}
}

/// An error and each of its sources, joined with `: `, skipping a source
/// whose message the error already includes.
fn chain(error: &dyn Error) -> String {
	let mut message = error.to_string();
	let mut source = error.source();
	while let Some(cause) = source {
		let text = cause.to_string();
		if !message.contains(&text) {
			message.push_str(": ");
			message.push_str(&text);
		}
		source = cause.source();
	}
	message
}

#[cfg(test)]
mod tests {
	use std::process::Command;

	use source_sentry_core::model::RepoPath;
	use source_sentry_core::ports::Canonical;

	use super::*;

	/// The advice for a file that changed to CRLF in the index while a plan
	/// was applied repeats `-C` and names the file from where git starts.
	#[test]
	fn the_renormalize_advice_of_a_refused_plan_runs_where_printed() {
		let directory = tempfile::tempdir().unwrap();
		let root = directory.path();
		std::fs::create_dir_all(root.join("docs")).unwrap();
		std::fs::create_dir_all(root.join("src")).unwrap();
		std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
		for args in [&["init", "--quiet"][..], &["add", "src/a.rs"]] {
			let status = Command::new("git")
				.args(args)
				.current_dir(root)
				.env_remove("GIT_DIR")
				.env_remove("GIT_WORK_TREE")
				.env_remove("GIT_INDEX_FILE")
				.status()
				.unwrap();
			assert!(status.success());
		}
		let docs = root.join("docs");
		let workspace = Workspace::open(Some(&docs)).unwrap();
		let error = ApplyError::Unverifiable {
			path: RepoPath::new("src/a.rs").unwrap(),
			reason: Canonical::CrlfInIndex,
		};
		let failure = Failure::apply(&workspace, &error);
		assert_eq!(failure.code, OPERATIONAL);
		let expected = workspace.git_command(
			&["add", "--renormalize"],
			&[&RepoPath::new("src/a.rs").unwrap()],
		);
		assert!(expected.contains("-- ../src/a.rs"), "{expected}");
		assert!(failure.message.contains(&expected), "{}", failure.message);
		assert!(
			failure.message.starts_with("cannot verify ../src/a.rs:"),
			"{}",
			failure.message
		);
	}
}
