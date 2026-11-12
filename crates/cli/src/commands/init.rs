//! `init`: adoption. Without paths it
//! only previews roots, claims and candidates; with paths, or `--all`, it
//! registers candidates as unattested.

use std::fmt::Write as _;

use source_sentry_core::mutate::{
	Candidates, InitPreview, InitRequest, InitSummary, Placement, init,
	init_preview,
};

use crate::cli::InitArgs;
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `init` on the working tree.
///
/// # Errors
///
/// The refusals of the planner and of confirmation; see [`crate::exit`].
pub(super) fn run(
	workspace: &Workspace,
	args: &InitArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (configs, inventory) = load(workspace, &snapshot)?;
	if args.paths.is_empty() && !args.all {
		printer.print(&preview(workspace, &init_preview(&configs, &inventory)));
		return Ok(exit::SUCCESS);
	}
	let candidates = if args.all {
		Candidates::All
	} else {
		Candidates::Only(
			args
				.paths
				.iter()
				.map(|path| workspace.repo_path(path))
				.collect::<Result<_, _>>()?,
		)
	};
	let request = InitRequest {
		candidates,
		classes: args.classes.clone(),
		owners: args.owners.clone(),
	};
	let planned = init(&snapshot, &configs, &inventory, &request)
		.map_err(|error| Failure::mutate(workspace, &error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&summary(workspace, &planned.summary),
		args.confirm,
		printer,
	)
}

/// The preview: roots, the candidates in full, and the documents already
/// tracked, counted by placement, because `init` only acts on candidates.
fn preview(workspace: &Workspace, preview: &InitPreview) -> String {
	let mut text = String::new();
	let roots: Vec<String> = preview
		.roots
		.iter()
		.map(|path| workspace.display_path(path))
		.collect();
	let _ = writeln!(
		text,
		"{}  {}",
		output::bold("Roots"),
		if roots.is_empty() {
			"none yet: a directory with a lockfile, or a config with `root: true`, \
			 is a root"
				.to_owned()
		} else {
			roots.join(", ")
		}
	);
	let mut placements = std::collections::BTreeMap::<String, usize>::new();
	for claim in &preview.claims {
		let placement = match &claim.placement {
			Placement::OnFile => "on-file".to_owned(),
			Placement::Lockfile { directory } => {
				format!("lockfile:{}", workspace.display_path(directory))
			}
			Placement::Remote { owner } => format!("remote:{owner}"),
		};
		*placements.entry(placement).or_default() += 1;
	}
	let tracked: Vec<String> = placements
		.iter()
		.map(|(placement, count)| format!("{count} {placement}"))
		.collect();
	let _ = writeln!(
		text,
		"{}  {}",
		output::bold("Tracked"),
		if tracked.is_empty() {
			"none yet".to_owned()
		} else {
			format!("{} ({})", preview.claims.len(), tracked.join(", "))
		}
	);
	let _ = writeln!(
		text,
		"\n{} ({})",
		output::bold("Candidates"),
		preview.candidates.len()
	);
	for candidate in &preview.candidates {
		let _ = writeln!(
			text,
			"  {}  in root {}",
			workspace.display_path(&candidate.path),
			workspace.display_path(&candidate.root)
		);
	}
	if !preview.candidates.is_empty() {
		let command = workspace.command_prefix();
		let _ = write!(
			text,
			"\nRegister candidates as unattested with `{command} init <path>...` or `{command} init --all`."
		);
	}
	text
}

/// What adoption registers.
fn summary(workspace: &Workspace, summary: &InitSummary) -> String {
	let mut text = format!(
		"Register {} as unattested\n",
		describe::count(summary.registered.len(), "document", "documents")
	);
	for registered in &summary.registered {
		super::track::registration(workspace, &mut text, registered);
	}
	super::track::carried(workspace, &mut text, &summary.carried);
	text
}
