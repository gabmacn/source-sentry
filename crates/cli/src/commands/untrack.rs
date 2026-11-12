//! `untrack`: remove declared sources from a document's declaration.

use std::fmt::Write as _;

use source_sentry_core::model::SourceDecl;
use source_sentry_core::mutate::{self, UntrackRequest, UntrackSummary};

use crate::cli::UntrackArgs;
use crate::describe;
use crate::exit::Failure;
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `untrack` on the working tree.
///
/// # Errors
///
/// Exit 64 without a source, and the refusals of the planner and of
/// confirmation; see [`crate::exit`].
pub(super) fn run(
	workspace: &Workspace,
	args: &UntrackArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	if args.ordered.is_empty() {
		return Err(Failure::usage(
			"name the sources to remove with --source or --pattern; to stop \
			 tracking a document, use `retire`",
		));
	}
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let request = UntrackRequest {
		document: workspace.document(&args.document)?,
		sources: args.ordered.clone(),
	};
	let planned = mutate::untrack(&snapshot, &inventory, &request)
		.map_err(|error| Failure::mutate(&error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&summary(&planned.summary),
		args.confirm,
		printer,
	)
}

/// What `untrack` removes.
fn summary(summary: &UntrackSummary) -> String {
	let mut text = format!(
		"Remove sources from the declaration of {}, {} ({})\n",
		output::bold(&summary.document.to_string()),
		super::track::placement_name(&summary.placement),
		describe::location(&summary.declared_in)
	);
	for source in &summary.removed {
		let (kind, declared) = match source {
			SourceDecl::Path(path) => ("source ", &path.path),
			SourceDecl::Pattern(pattern) => ("pattern", &pattern.pattern),
		};
		let _ = writeln!(text, "  {kind}  {declared}");
	}
	let _ = writeln!(
		text,
		"{} remain",
		describe::count(summary.declaration.sources.len(), "source", "sources")
	);
	text
}
