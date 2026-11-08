//! `migrate`: lockfile schema migration.

use std::fmt::Write as _;

use source_sentry_core::mutate::{SchemaReport, migrate};

use crate::cli::MigrateArgs;
use crate::exit::Failure;
use crate::output::Printer;
use crate::workspace::{Workspace, load};

/// Runs `migrate` on the working tree. Lockfile schema 1 is the only one so
/// far, so the plan is empty today.
///
/// # Errors
///
/// Exit 3 for a lockfile whose schema cannot be read or is unknown, and the
/// refusals of confirmation; see [`crate::exit`].
pub(super) fn run(
	workspace: &Workspace,
	args: &MigrateArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let planned =
		migrate(&snapshot, &inventory).map_err(|error| Failure::mutate(&error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&summary(&planned.summary),
		args.confirm,
		printer,
	)
}

/// Every lockfile's schema.
fn summary(report: &SchemaReport) -> String {
	let mut text = format!("Lockfile schema {} is current.\n", report.current);
	if report.lockfiles.is_empty() {
		text.push_str("There are no lockfiles.\n");
	}
	for lockfile in &report.lockfiles {
		let _ = writeln!(text, "  {}  schema {}", lockfile.path, lockfile.schema);
	}
	text
}
