//! Re-expressing sources and carrying reviewed references through a rename.

use super::{RenameSourceKind, RenamedSource};
use crate::claims::SourceGlob;
use crate::model::{
	Declaration, DocumentId, RebaseError, RepoPath, SourceDecl, rebase_path,
	rebase_pattern,
};
use crate::mutate::MutateError;

/// Re-expresses every relative spelling, including reviewed members, for
/// the renamed document's new directory. Rooted spellings stay rooted.
pub(in crate::mutate) fn rebase(
	declaration: &mut Declaration,
	old_project: &RepoPath,
	new_project: &RepoPath,
	old: &RepoPath,
	new: &RepoPath,
	changes: &mut Vec<RenamedSource>,
) -> Result<(), MutateError> {
	let from = old.parent().unwrap_or(RepoPath::ROOT);
	let to = new.parent().unwrap_or(RepoPath::ROOT);
	let document = DocumentId::Local(new.clone());
	for source in &mut declaration.sources {
		match source {
			SourceDecl::Path(source) => {
				let after = rebase_spelling(
					&source.path,
					old_project,
					new_project,
					&from,
					&to,
					false,
				)
				.map_err(|error| cannot_rebase(&document, &source.path, error))?;
				record(
					changes,
					&document,
					&mut source.path,
					after,
					RenameSourceKind::Path,
				);
			}
			SourceDecl::Pattern(source) => {
				let after = rebase_spelling(
					&source.pattern,
					old_project,
					new_project,
					&from,
					&to,
					true,
				)
				.map_err(|error| cannot_rebase(&document, &source.pattern, error))?;
				record(
					changes,
					&document,
					&mut source.pattern,
					after,
					RenameSourceKind::Pattern,
				);
				for member in &mut source.members {
					let after = rebase_spelling(
						&member.path,
						old_project,
						new_project,
						&from,
						&to,
						false,
					)
					.map_err(|error| cannot_rebase(&document, &member.path, error))?;
					record(
						changes,
						&document,
						&mut member.path,
						after,
						RenameSourceKind::Member,
					);
				}
			}
		}
	}
	Ok(())
}

/// Rewrites path sources and reviewed pattern members that resolve to the
/// old document. Unmatched patterns keep their reviewed membership, so the
/// next check reports the removal. Conflicting destination reviews refuse
/// the plan instead of discarding either review.
pub(super) fn update(
	project: &RepoPath,
	document: &DocumentId,
	declaration: &mut Declaration,
	old: &RepoPath,
	new: &RepoPath,
	changes: &mut Vec<RenamedSource>,
) -> Result<bool, MutateError> {
	let mut modified = false;
	for source in &mut declaration.sources {
		match source {
			SourceDecl::Path(source)
				if resolve(project, document, &source.path).as_ref() == Some(old) =>
			{
				let after = spelling(project, document, &source.path, new)?;
				record(
					changes,
					document,
					&mut source.path,
					after,
					RenameSourceKind::Path,
				);
				modified = true;
			}
			SourceDecl::Pattern(source) => {
				let Some(index) = source.members.iter().position(|member| {
					resolve(project, document, &member.path).as_ref() == Some(old)
				}) else {
					continue;
				};
				let local = match document {
					DocumentId::Local(path) => Some(path),
					DocumentId::Remote(_) => None,
				};
				let matches =
					SourceGlob::compile_project(project, local, &source.pattern)
						.is_ok_and(|glob| glob.is_match(new));
				if !matches {
					changes.push(RenamedSource {
						document: document.clone(),
						before: source.pattern.clone(),
						after: source.pattern.clone(),
						kind: RenameSourceKind::UnmatchedPattern,
					});
					continue;
				}
				if source.members.iter().any(|member| {
					resolve(project, document, &member.path).as_ref() == Some(new)
				}) {
					return Err(MutateError::RenameMemberConflict {
						document: document.clone(),
						pattern: source.pattern.clone(),
						path: new.clone(),
					});
				}
				let member = &mut source.members[index];
				let after = spelling(project, document, &member.path, new)?;
				record(
					changes,
					document,
					&mut member.path,
					after,
					RenameSourceKind::Member,
				);
				modified = true;
			}
			SourceDecl::Path(_) => {}
		}
	}
	Ok(modified)
}

/// Resolves a spelling without changing invalid sources unrelated to this
/// rename; verification retains responsibility for their diagnostics.
fn resolve(
	project: &RepoPath,
	document: &DocumentId,
	spelling: &str,
) -> Option<RepoPath> {
	match document {
		DocumentId::Local(path) => {
			crate::model::resolve_project_source(project, Some(path), spelling).ok()
		}
		DocumentId::Remote(_) => {
			crate::model::resolve_project_source(project, None, spelling).ok()
		}
	}
}

/// Expresses a new target with the same rooted or relative convention.
fn spelling(
	project: &RepoPath,
	document: &DocumentId,
	before: &str,
	new: &RepoPath,
) -> Result<String, MutateError> {
	if before.starts_with(":/") {
		return Ok(format!(":/{new}"));
	}
	if before.starts_with('/') {
		return Ok(
			new
				.strip_prefix(project)
				.map_or_else(|| format!(":/{new}"), |relative| format!("/{relative}")),
		);
	}
	let directory = match document {
		DocumentId::Local(path) => path.parent().unwrap_or(RepoPath::ROOT),
		DocumentId::Remote(_) => return Ok(format!("/{new}")),
	};
	rebase_path(new.as_str(), &RepoPath::ROOT, &directory)
		.map_err(|error| cannot_rebase(document, before, error))
}

/// Names the source whose re-expression failed.
fn cannot_rebase(
	document: &DocumentId,
	declared: &str,
	error: RebaseError,
) -> MutateError {
	MutateError::CannotRenameSource {
		document: document.clone(),
		declared: declared.to_owned(),
		error,
	}
}

/// Records a changed spelling before replacing it, without touching its
/// oid or review metadata.
fn record(
	changes: &mut Vec<RenamedSource>,
	document: &DocumentId,
	before: &mut String,
	after: String,
	kind: RenameSourceKind,
) {
	if *before != after {
		changes.push(RenamedSource {
			document: document.clone(),
			before: std::mem::replace(before, after.clone()),
			after,
			kind,
		});
	}
}

/// Re-expresses rooted paths for a new project and relative paths for a new directory.
fn rebase_spelling(
	declared: &str,
	old_project: &RepoPath,
	new_project: &RepoPath,
	from: &RepoPath,
	to: &RepoPath,
	pattern: bool,
) -> Result<String, RebaseError> {
	if declared.starts_with(":/")
		|| (declared.starts_with('/') && old_project == new_project)
	{
		return Ok(declared.to_owned());
	}
	if declared.starts_with('/') {
		let rooted = if pattern {
			SourceGlob::compile_project(old_project, None, declared)
				.map_err(|error| RebaseError::Glob(error.to_string()))?
				.rooted()
		} else {
			format!(
				"/{}",
				crate::model::resolve_project_source(old_project, None, declared)?
			)
		};
		let target = &rooted[1..];
		let relative = if new_project.is_root() {
			Some(target)
		} else {
			target
				.strip_prefix(new_project.as_str())
				.and_then(|rest| rest.strip_prefix('/'))
		};
		return Ok(relative.map_or_else(
			|| format!(":{rooted}"),
			|relative| format!("/{relative}"),
		));
	}
	if pattern {
		rebase_pattern(declared, from, to)
	} else {
		rebase_path(declared, from, to)
	}
}
