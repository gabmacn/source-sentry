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

use source_sentry_core::config::LoadError;
use source_sentry_core::mutate::MutateError;
use source_sentry_core::policy::USAGE_EXIT_CODE;
use source_sentry_core::ports::BackendError;
use source_sentry_git::ApplyError;

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
	pub(crate) fn load(error: &LoadError) -> Self {
		match error {
			LoadError::Backend(error) => Self::backend(error),
			LoadError::Invalid(errors) => {
				let mut message = String::from("the configs are invalid:");
				for error in errors {
					let _ = write!(message, "\n  - {error}");
				}
				Self::new(CONFIGURATION, message)
			}
		}
	}

	/// A planner's refusal, with the code its cause calls for.
	pub(crate) fn mutate(error: &MutateError) -> Self {
		Self::new(refusal_code(error), chain(error))
	}

	/// A write plan that could not be applied: exit 4.
	///
	/// A plan that creates a file which already exists usually means a file
	/// that an earlier command created and git does not track yet: snapshots
	/// list git's index, so the hint says to add it.
	pub(crate) fn apply(error: &ApplyError) -> Self {
		let mut message = chain(error);
		if !matches!(error, ApplyError::RollbackFailed { .. }) {
			message.push_str("; nothing was changed");
		}
		if let ApplyError::Stale {
			path,
			expected: None,
			actual: Some(_),
		} = error
		{
			let _ = write!(
				message,
				". A file that git does not track already exists at {path}; move \
				 it aside, or add it with `git add {path}`, and run the command \
				 again"
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
