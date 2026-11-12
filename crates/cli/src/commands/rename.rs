//! `rename`: update records after a document has been moved in git.

use std::{fmt::Write as _, fs, io};

use source_sentry_core::model::DocumentId;
use source_sentry_core::mutate::{
	RenameRequest, RenameSourceKind, RenameSummary, rename,
};

use crate::cli::RenameArgs;
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::Printer;
use crate::workspace::{Workspace, load};

/// Plans, confirms and applies a rename using filesystem document paths.
pub(super) fn run(
	workspace: &Workspace,
	args: &RenameArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let request = RenameRequest {
		old: workspace.document(&args.old)?,
		new: workspace.document(&args.new)?,
	};
	check_files(workspace, &request)?;
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (configs, inventory) = load(workspace, &snapshot)?;
	let planned = rename(
		&snapshot,
		&workspace.repository,
		&configs,
		&inventory,
		&request,
	)
	.map_err(|error| Failure::mutate(workspace, &error))?;
	super::apply::confirm_and_apply(
		workspace,
		&planned.plan,
		&summary(workspace, &planned.summary),
		args.confirm,
		printer,
	)
}

/// Checks physical existence independently of git membership: an untracked
/// old file is still present, and a sparse destination is not a moved file.
fn check_files(
	workspace: &Workspace,
	request: &RenameRequest,
) -> Result<(), Failure> {
	let (DocumentId::Local(old), DocumentId::Local(new)) =
		(&request.old, &request.new)
	else {
		return Err(Failure::new(
			exit::REFUSED,
			"remote identities cannot be renamed; name two filesystem paths",
		));
	};
	let root = workspace.repository.work_dir().ok_or_else(|| {
		Failure::new(exit::OPERATIONAL, "rename requires a working tree")
	})?;
	for (path, should_exist) in [(old, false), (new, true)] {
		let exists = match fs::symlink_metadata(root.join(path.as_str())) {
			Ok(_) => true,
			Err(error) if error.kind() == io::ErrorKind::NotFound => false,
			Err(error) => {
				return Err(Failure::new(
					exit::OPERATIONAL,
					format!("cannot inspect `{}`: {error}", workspace.display_path(path)),
				));
			}
		};
		if exists != should_exist {
			return Err(Failure::new(
				exit::REFUSED,
				format!(
					"{}; move the file first with `git mv {} {}`",
					if should_exist {
						"the new file does not exist"
					} else {
						"the old file still exists"
					},
					super::shell_word(&workspace.display_path(old)),
					super::shell_word(&workspace.display_path(new))
				),
			));
		}
	}
	Ok(())
}

/// The declaration move, preserved metadata, source rewrites and series
/// changes, displayed from the current directory.
fn summary(workspace: &Workspace, summary: &RenameSummary) -> String {
	let mut text = format!(
		"Rename {} to {}\n  declaration: {} -> {}\n  keep every source review, document review, seal and completion\n  old tombstones stay in place\n",
		workspace.display_path(&summary.old),
		workspace.display_path(&summary.new),
		describe::location(workspace, &summary.from),
		describe::location(workspace, &summary.to)
	);
	let _ = writeln!(
		text,
		"  projects: {} -> {}",
		workspace.display_path(&summary.project_before),
		workspace.display_path(&summary.project_after)
	);
	super::track::settings_preview(&mut text, &summary.settings);
	source_changes(workspace, &mut text, &summary.sources);
	if summary.series_before != summary.series_after {
		if let (Some(before), Some(after)) =
			(&summary.series_before, &summary.series_after)
			&& before.scope == after.scope
			&& before.name == after.name
		{
			let _ = writeln!(
				text,
				"  change series `{}` ({}) from version {} to {}",
				before.name, before.scope, before.version, after.version
			);
			return text;
		}
		if let Some(before) = &summary.series_before {
			let _ = writeln!(
				text,
				"  leave series `{}` ({}) at version {}",
				before.name, before.scope, before.version
			);
		}
		if let Some(after) = &summary.series_after {
			let _ = writeln!(
				text,
				"  join series `{}` ({}) at version {}",
				after.name, after.scope, after.version
			);
		}
	}
	text
}

/// Lists every rewritten source and reviewed member before confirmation.
pub(super) fn source_changes(
	workspace: &Workspace,
	text: &mut String,
	sources: &[source_sentry_core::mutate::RenamedSource],
) {
	for source in sources {
		let document = workspace.display_document(&source.document);
		match source.kind {
			RenameSourceKind::UnmatchedPattern => {
				let _ = writeln!(
					text,
					"  {document}: pattern `{}` no longer matches the renamed member; leave it unchanged for `check`",
					source.before
				);
			}
			kind => {
				let label = match kind {
					RenameSourceKind::Path => "path",
					RenameSourceKind::Pattern | RenameSourceKind::UnmatchedPattern => {
						"pattern"
					}
					RenameSourceKind::Member => "reviewed member (keep review)",
				};
				let _ = writeln!(
					text,
					"  {document}: {label} `{}` -> `{}`",
					source.before, source.after
				);
			}
		}
	}
}
