//! `attest`: record that a document was reviewed against its sources.

use std::fmt::Write as _;

use source_sentry_core::mutate::{
	AttestRequest, AttestSummary, Selection, attest,
};

use crate::cli::AttestArgs;
use crate::describe;
use crate::exit::Failure;
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `attest` on the working tree. Without `--expect`, the current state
/// is recorded, and the summary reports what was bound. The
/// review is dated when `attest` runs, and cannot be backdated.
///
/// # Errors
///
/// The refusals of the planner and of confirmation; see
/// [`crate::exit`].
pub(super) fn run(
	workspace: &Workspace,
	args: &AttestArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let request = AttestRequest {
		document: workspace.document(&args.document)?,
		selection: if args.sources.is_empty() {
			Selection::All
		} else {
			Selection::Sources(args.sources.clone())
		},
		expectations: (!args.expectations.is_empty())
			.then(|| args.expectations.clone()),
		reviewer: super::person(workspace, args.reviewer.as_deref())?,
		at: super::now(),
		reread: args.reread,
	};
	let planned = attest(&snapshot, &inventory, &request)
		.map_err(|error| Failure::mutate(&error))?;
	let summary = summary(&planned.summary, !args.expectations.is_empty());
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&summary,
		args.confirm,
		printer,
	)
}

/// What the attestation records.
fn summary(summary: &AttestSummary, expected: bool) -> String {
	let mut text = format!(
		"Attest {} ({})\n",
		output::bold(&summary.document.to_string()),
		describe::location(&summary.declared_in)
	);
	for source in &summary.sources {
		let kind = if source.pattern { " (pattern)" } else { "" };
		let _ = writeln!(text, "  {}{kind}", source.declared);
		for entry in &source.entries {
			let previous = entry
				.previous
				.as_ref()
				.map_or("never attested", describe::short);
			let recorded = entry
				.recorded
				.as_ref()
				.map_or("dropped from the pattern", describe::short);
			let _ = writeln!(text, "    {}  {previous} → {recorded}", entry.path);
		}
	}
	let reviewer = summary
		.review
		.by
		.as_deref()
		.unwrap_or("an unknown reviewer");
	let _ = writeln!(
		text,
		"Reviewed by {reviewer} at {}",
		describe::instant(summary.review.at)
	);
	if summary.reread {
		text.push_str("The whole document was reread.\n");
	}
	if summary.completed.is_some() {
		text.push_str("This completes the series entry.\n");
	}
	let verb = if expected {
		"Matches the expectations"
	} else {
		"Binds the current state"
	};
	let _ = writeln!(text, "{verb}:");
	for expectation in &summary.expectations {
		let _ = writeln!(text, "  {expectation}");
	}
	text
}
