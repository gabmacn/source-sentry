//! Integrity checks that discovery alone does not perform.

use super::super::repo::Repo;
use super::{RenameSeries, membership};
use crate::claims::{
	Claim, Diagnostic, DiagnosticKind, Inventory, Location, MemberState,
	SeriesMember, SeriesMembers,
};
use crate::config::ConfigSet;
use crate::model::{DocumentId, Lifecycle, RepoPath};
use crate::mutate::MutateError;
use crate::ports::Snapshot;

/// Refuses final errors about the destination, a changed sealed record,
/// and living-series heads newly left missing, sealed or removed by the
/// rename.
///
/// `configs` are those of the project `old` belonged to, which place it in
/// its series before the file moved; see [`heads`].
pub(super) fn integrity<S: Snapshot>(
	repo: &Repo<'_, S>,
	after: &Inventory,
	renamed: &Claim,
	configs: &ConfigSet,
	old: &RepoPath,
	new: &RepoPath,
) -> Result<(), MutateError> {
	let mut diagnostics: Vec<_> = after
		.diagnostics
		.iter()
		.filter(|diagnostic| {
			diagnostic.severity().is_fatal()
				&& (diagnostic.document.as_ref() == Some(&renamed.document)
					|| (diagnostic.kind == DiagnosticKind::DuplicateVersion
						&& (diagnostic.location.file == *new
							|| diagnostic
								.related
								.iter()
								.any(|location| location.file == *new))))
		})
		.cloned()
		.collect();
	if let Some(seal) = &renamed.declaration.seal {
		let oid = repo.oid(new)?;
		if oid != seal.oid {
			diagnostics.push(Diagnostic::new(DiagnosticKind::BrokenSeal, renamed.location(), format!("`{new}` differs from its seal. Restore the sealed content before renaming the record")).about(renamed.document.clone()));
		}
	}
	let membership = membership(repo.inventory, old, configs);
	heads(
		repo,
		after,
		renamed,
		(old, membership.as_ref()),
		new,
		&mut diagnostics,
	);
	if diagnostics.is_empty() {
		Ok(())
	} else {
		Err(MutateError::WouldIntroduce { diagnostics })
	}
}

/// Living-series heads newly left missing, sealed or removed by the rename.
///
/// The old path in `moved` counts as an entry of its series even when the
/// inventory read before the rename no longer lists it, as for an on-file
/// declaration that has already travelled with its file, so that the
/// outcome does not depend on where the declaration is written.
fn heads<S: Snapshot>(
	repo: &Repo<'_, S>,
	after: &Inventory,
	renamed: &Claim,
	moved: (&RepoPath, Option<&RenameSeries>),
	new: &RepoPath,
	diagnostics: &mut Vec<Diagnostic>,
) {
	for series in &after.series {
		if series.series.lifecycle != Lifecycle::Living {
			continue;
		}
		let old = repo.inventory.series.iter().find(|old| {
			old.series.scope == series.series.scope
				&& old.series.name == series.series.name
		});
		let previous = old.and_then(|old| old.members.last());
		let moved_head = moved_head(series, previous, moved);
		let Some(head) = series.members.last() else {
			if let Some(previous) =
				moved_head.or_else(|| previous.map(|previous| &previous.path))
			{
				diagnostics.push(headless(series, previous, new));
			}
			continue;
		};
		let document = DocumentId::Local(head.path.clone());
		let sealed = after.claims_of(&document).any(Claim::is_sealed)
			|| after.tombstones.iter().any(|tombstone| {
				tombstone.document == document && tombstone.retired.seal.is_some()
			});
		let unchanged_head =
			moved_head.is_none() && previous.is_some_and(|old| old == head);
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
}

/// The old path, when it was the newest entry of `series` before the file
/// moved, whether or not `previous`, the newest entry still listed before
/// the rename, includes it.
fn moved_head<'a>(
	series: &SeriesMembers,
	previous: Option<&SeriesMember>,
	(old, membership): (&'a RepoPath, Option<&RenameSeries>),
) -> Option<&'a RepoPath> {
	let membership = membership.filter(|membership| {
		membership.scope == series.series.scope
			&& membership.name == series.series.name
	})?;
	previous
		.is_none_or(|previous| {
			&previous.path == old || membership.version > previous.version
		})
		.then_some(old)
}

/// A living series that the rename would leave with no entry at all. Its
/// head moves out of the series, which, like retiring the head, requires
/// retiring the series itself or adding a successor first.
fn headless(
	series: &SeriesMembers,
	head: &RepoPath,
	new: &RepoPath,
) -> Diagnostic {
	let config = series.series.origin.file.clone();
	let defined_in = config
		.as_ref()
		.map_or_else(|| "the built-in defaults".to_owned(), ToString::to_string);
	Diagnostic::new(
		DiagnosticKind::MissingHead,
		Location::field(
			config.unwrap_or_else(|| head.clone()),
			format!("series.{}", series.series.name),
		),
		format!(
			"`{head}` is the living head of the series `{name}`, and `{new}` is \
			 outside the series, so the rename would leave it with no head. \
			 Moving it out requires retiring the series, which is defined in \
			 {defined_in}: remove the series there first. A head created by \
			 mistake is replaced by adding a new successor",
			name = series.series.name,
		),
	)
	.about(DocumentId::Local(head.clone()))
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
