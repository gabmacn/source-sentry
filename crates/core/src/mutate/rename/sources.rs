//! Re-expressing sources and carrying reviewed references through a rename.

use super::{RenameSourceKind, RenamedSource};
use crate::claims::SourceGlob;
use crate::model::{
	Declaration, DocumentId, RebaseError, RepoPath, SourceDecl, rebase_path,
	rebase_pattern, resolve_rooted, resolve_source,
};
use crate::mutate::MutateError;

/// Re-expresses every relative spelling, including reviewed members, for
/// the renamed document's new directory. Rooted spellings stay rooted.
pub(super) fn rebase(
	declaration: &mut Declaration,
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
				let after = rebase_path(&source.path, &from, &to)
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
				let after = rebase_pattern(&source.pattern, &from, &to)
					.map_err(|error| cannot_rebase(&document, &source.pattern, error))?;
				record(
					changes,
					&document,
					&mut source.pattern,
					after,
					RenameSourceKind::Pattern,
				);
				for member in &mut source.members {
					let after = rebase_path(&member.path, &from, &to)
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
				if resolve(document, &source.path).as_ref() == Some(old) =>
			{
				let after = spelling(document, &source.path, new)?;
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
					resolve(document, &member.path).as_ref() == Some(old)
				}) else {
					continue;
				};
				let local = match document {
					DocumentId::Local(path) => Some(path),
					DocumentId::Remote(_) => None,
				};
				let matches = SourceGlob::compile(local, &source.pattern)
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
				if source
					.members
					.iter()
					.any(|member| resolve(document, &member.path).as_ref() == Some(new))
				{
					return Err(MutateError::RenameMemberConflict {
						document: document.clone(),
						pattern: source.pattern.clone(),
						path: new.clone(),
					});
				}
				let member = &mut source.members[index];
				let after = spelling(document, &member.path, new)?;
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
fn resolve(document: &DocumentId, spelling: &str) -> Option<RepoPath> {
	match document {
		DocumentId::Local(path) => resolve_source(path, spelling).ok(),
		DocumentId::Remote(_) => resolve_rooted(spelling).ok(),
	}
}

/// Expresses a new target with the same rooted or relative convention.
fn spelling(
	document: &DocumentId,
	before: &str,
	new: &RepoPath,
) -> Result<String, MutateError> {
	if before.starts_with('/') {
		return Ok(format!("/{new}"));
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
