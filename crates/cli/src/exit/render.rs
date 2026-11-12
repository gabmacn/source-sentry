//! Human error locations, without changing declared spellings.

use std::fmt::Write as _;

use source_sentry_core::claims::Diagnostic;
use source_sentry_core::config::ConfigError;
use source_sentry_core::mutate::MutateError;

use crate::describe;
use crate::workspace::Workspace;

/// A configuration error with its file and import chain displayed locally.
pub(super) fn config(workspace: &Workspace, error: &ConfigError) -> String {
	let mut text = workspace.display_path(&error.file);
	if let Some(position) = error.position {
		let _ = write!(text, ":{position}");
	}
	text.push_str(": ");
	if let Some(setting) = &error.setting {
		let _ = write!(text, "`{setting}`: ");
	}
	let _ = write!(text, "{}", error.kind);
	if !error.import_chain.is_empty() {
		let chain: Vec<_> = error
			.import_chain
			.iter()
			.map(|path| workspace.display_path(path))
			.collect();
		let _ = write!(text, " (imported through {})", chain.join(" → "));
	}
	if let Some(correction) = &error.correction {
		let _ = write!(text, ". {correction}");
	}
	text
}

/// Diagnostics in a planner refusal, with current-directory locations.
fn diagnostics(workspace: &Workspace, diagnostics: &[Diagnostic]) -> String {
	diagnostics
		.iter()
		.map(|diagnostic| {
			format!(
				"  - {}: {}",
				describe::location(workspace, &diagnostic.location),
				diagnostic.message
			)
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// A planner refusal whose diagnostic locations belong to this invocation.
pub(super) fn mutation(workspace: &Workspace, error: &MutateError) -> String {
	match error {
		MutateError::RenameRefused { old, new, reason } => format!(
			"cannot rename `{}` to `{}`: {reason}",
			workspace.display_document(old),
			workspace.display_document(new)
		),
		MutateError::CannotRenameSource {
			document,
			declared,
			error,
		} => format!(
			"cannot rename `{}`: cannot re-express source `{declared}`: {error}",
			workspace.display_document(document)
		),
		MutateError::RenameMemberConflict {
			document,
			pattern,
			path,
		} => format!(
			"the pattern `{pattern}` of `{}` already reviews `{}`. Run `diff` and review the pattern's membership before renaming; neither review was discarded",
			workspace.display_document(document),
			workspace.display_path(path)
		),
		MutateError::DocumentHasErrors {
			document,
			diagnostics: errors,
		} => format!(
			"`{}` has errors to fix first:\n{}",
			workspace.display_document(document),
			diagnostics(workspace, errors)
		),
		MutateError::WouldIntroduce {
			diagnostics: errors,
		} => format!(
			"the change would introduce these errors, so nothing was planned:\n{}",
			diagnostics(workspace, errors)
		),
		MutateError::InvalidConfigs { errors } => format!(
			"the configs would no longer load after the change, so nothing was planned:\n{}",
			errors
				.iter()
				.map(|error| format!("  - {}", config(workspace, error)))
				.collect::<Vec<_>>()
				.join("\n")
		),
		MutateError::DoubleClaim { document, sites } => format!(
			"`{}` is claimed {} times ({}). Keep one declaration and remove the others first",
			workspace.display_document(document),
			sites.len(),
			sites
				.iter()
				.map(|site| describe::location(workspace, site))
				.collect::<Vec<_>>()
				.join(", ")
		),
		MutateError::AlreadyClaimed { document, sites } => format!(
			"`{}` is already tracked ({}), and a second claim would be a double claim. Use `track --from` to move it",
			workspace.display_document(document),
			sites
				.iter()
				.map(|site| describe::location(workspace, site))
				.collect::<Vec<_>>()
				.join(", ")
		),
		_ => super::chain(error),
	}
}
