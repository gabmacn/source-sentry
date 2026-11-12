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

mod mutation;

/// A planner refusal whose structured paths belong to this invocation.
pub(super) fn mutation(workspace: &Workspace, error: &MutateError) -> String {
	mutation::render(workspace, error)
}
