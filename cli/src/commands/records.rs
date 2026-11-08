//! `seal` and `retire`: records and tombstones.

use std::fmt::Write as _;

use source_sentry_core::mutate::{
	self, RetireRequest, RetireSummary, SealRequest, SealSummary,
};

use crate::cli::RecordArgs;
use crate::describe;
use crate::exit::Failure;
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `seal` on the working tree. `--by` defaults to git's `user.name`.
///
/// # Errors
///
/// The refusals of the planner and of confirmation; see [`crate::exit`].
pub(super) fn seal(
	workspace: &Workspace,
	args: &RecordArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let request = SealRequest {
		document: workspace.document(&args.document)?,
		by: super::person(workspace, args.by.as_deref())?,
		at: super::now(),
	};
	let planned = mutate::seal(&snapshot, &inventory, &request)
		.map_err(|error| Failure::mutate(&error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&seal_summary(&planned.summary),
		args.confirm,
		printer,
	)
}

/// Runs `retire` on the working tree. `--by` defaults to git's `user.name`.
///
/// # Errors
///
/// The refusals of the planner and of confirmation; see [`crate::exit`].
pub(super) fn retire(
	workspace: &Workspace,
	args: &RecordArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let request = RetireRequest {
		document: workspace.document(&args.document)?,
		by: super::person(workspace, args.by.as_deref())?,
		at: super::now(),
	};
	let planned = mutate::retire(&snapshot, &inventory, &request)
		.map_err(|error| Failure::mutate(&error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&retire_summary(&planned.summary),
		args.confirm,
		printer,
	)
}

/// What sealing records.
fn seal_summary(summary: &SealSummary) -> String {
	let by = summary.seal.by.as_deref().unwrap_or("an unknown person");
	let mut text = format!(
		"Seal {} in {}\n",
		output::bold(summary.document.as_str()),
		summary.lockfile
	);
	let _ = writeln!(
		text,
		"  oid {}, by {by} at {}",
		summary.seal.oid,
		describe::instant(summary.seal.at)
	);
	text.push_str(
		"A sealed record must never change, and its sources are frozen.\n",
	);
	text
}

/// What retiring records.
fn retire_summary(summary: &RetireSummary) -> String {
	let review = &summary.tombstone.retired;
	let by = review.by.as_deref().unwrap_or("an unknown person");
	let mut text = format!(
		"Retire {}: its entry in {} becomes a tombstone\n",
		output::bold(&summary.document.to_string()),
		summary.lockfile
	);
	let _ = writeln!(text, "  by {by} at {}", describe::instant(review.at));
	if let Some(seal) = &summary.tombstone.seal {
		let _ = writeln!(text, "  keeps the seal {}", seal.oid);
	}
	text.push_str(
		"The document itself is not touched; deleting it is a separate change.\n",
	);
	text
}
