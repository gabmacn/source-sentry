//! Integrity checks that discovery alone does not perform.

use super::super::repo::Repo;
use crate::claims::{
	Claim, Diagnostic, DiagnosticKind, Inventory, Location, MemberState,
};
use crate::model::{DocumentId, Lifecycle, RepoPath};
use crate::mutate::MutateError;
use crate::ports::Snapshot;

/// Refuses final errors about the destination, a changed sealed record,
/// and living-series heads newly left missing or sealed by the rename.
pub(super) fn integrity<S: Snapshot>(
	repo: &Repo<'_, S>,
	after: &Inventory,
	renamed: &Claim,
	new: &RepoPath,
) -> Result<(), MutateError> {
	let mut diagnostics: Vec<_> = after
		.diagnostics
		.iter()
		.filter(|diagnostic| {
			diagnostic.severity().is_fatal()
				&& diagnostic.document.as_ref() == Some(&renamed.document)
		})
		.cloned()
		.collect();
	if let Some(seal) = &renamed.declaration.seal {
		let oid = repo.oid(new)?;
		if oid != seal.oid {
			diagnostics.push(Diagnostic::new(DiagnosticKind::BrokenSeal, renamed.location(), format!("`{new}` differs from its seal. Restore the sealed content before renaming the record")).about(renamed.document.clone()));
		}
	}
	for series in &after.series {
		if series.series.lifecycle != Lifecycle::Living {
			continue;
		}
		let Some(head) = series.members.last() else {
			continue;
		};
		let document = DocumentId::Local(head.path.clone());
		let sealed = after.claims_of(&document).any(Claim::is_sealed)
			|| after.tombstones.iter().any(|tombstone| {
				tombstone.document == document && tombstone.retired.seal.is_some()
			});
		let old = repo.inventory.series.iter().find(|old| {
			old.series.scope == series.series.scope
				&& old.series.name == series.series.name
		});
		let unchanged_head = old
			.and_then(|old| old.members.last())
			.is_some_and(|old| old == head);
		if unchanged_head && document != renamed.document {
			continue;
		}
		let location = Location::field(
			series
				.series
				.origin
				.file
				.clone()
				.unwrap_or_else(|| head.path.clone()),
			format!("series.{}", series.series.name),
		);
		if head.state != MemberState::Present {
			diagnostics.push(Diagnostic::new(DiagnosticKind::MissingHead, location.clone(), format!("the rename would leave the living series `{}` with a missing head. Restore its head or add a successor", series.series.name)).about(document.clone()));
		}
		if sealed {
			diagnostics.push(Diagnostic::new(DiagnosticKind::SealedHead, location, format!("the rename would leave `{}` as the sealed head of living series `{}`. Add a living successor first", head.path, series.series.name)).about(document));
		}
	}
	if diagnostics.is_empty() {
		Ok(())
	} else {
		Err(MutateError::WouldIntroduce { diagnostics })
	}
}

/// Checks the filesystem identities and git membership before planning a rename.
pub(super) fn paths<'a, S: Snapshot>(
	snapshot: &S,
	request: &'a super::RenameRequest,
) -> Result<(&'a RepoPath, &'a RepoPath), MutateError> {
	let (DocumentId::Local(old), DocumentId::Local(new)) =
		(&request.old, &request.new)
	else {
		return Err(super::refused(
			request,
			"remote identities cannot be renamed; name two filesystem paths",
		));
	};
	let tracked: std::collections::BTreeSet<_> =
		snapshot.paths()?.into_iter().collect();
	if tracked.contains(old) {
		return Err(super::refused(
			request,
			"the old file still exists; move the file first with `git mv <old> <new>`",
		));
	}
	if !tracked.contains(new) {
		return Err(super::refused(
			request,
			"the new file is absent or not tracked by git; move the file first with `git mv`, or add its path with `git add -N`",
		));
	}
	if crate::mutate::repo::is_own_file(new) {
		return Err(MutateError::NotADocument { path: new.clone() });
	}
	Ok((old, new))
}
