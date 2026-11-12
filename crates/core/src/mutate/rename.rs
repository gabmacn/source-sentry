//! Updating declarations after a document has been moved in git.

mod sources;
mod validate;

use super::edits::Edits;
use super::repo::{Repo, is_own_file};
use super::track::{location, rediscovered};
use super::verify::verify;
use super::{MutateError, Placement, Planned};
use crate::adapters::frontmatter;
use crate::claims::{DiagnosticKind, Inventory, Location, Mode, Version};
use crate::config::{ConfigSet, Scope};
use crate::model::{Declaration, DocumentId, RepoPath};
use crate::ports::{History, Revision, Snapshot};

/// The old and new filesystem identities, resolved by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameRequest {
	/// The declared document whose file has already been moved.
	pub old: DocumentId,
	/// Its tracked destination, without an independent declaration.
	pub new: DocumentId,
}

/// One source spelling or reviewed member affected by a rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenamedSource {
	/// The document whose declaration changes, using its destination path.
	pub document: DocumentId,
	/// The old spelling.
	pub before: String,
	/// The new spelling, or the unchanged spelling for an unmatched pattern.
	pub after: String,
	/// What the plan does with this source.
	pub kind: RenameSourceKind,
}

/// How a source is affected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameSourceKind {
	/// A path source is re-expressed or follows the renamed document.
	Path,
	/// A pattern is re-expressed for the moved document's directory.
	Pattern,
	/// A reviewed pattern member follows the document and keeps its review.
	Member,
	/// A pattern no longer matches the renamed member and remains unchanged.
	UnmatchedPattern,
}

/// The document's place in a series before or after the rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSeries {
	/// The scope in which the series is defined.
	pub scope: Scope,
	/// Its name.
	pub name: String,
	/// The parsed version at this path.
	pub version: Version,
}

/// Everything the rename preserves or changes, for confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSummary {
	/// The old path.
	pub old: RepoPath,
	/// The destination path.
	pub new: RepoPath,
	/// Where the declaration was written before the file moved.
	pub from: Location,
	/// Where the declaration will be written.
	pub to: Location,
	/// Each source spelling and reviewed member affected by the move.
	pub sources: Vec<RenamedSource>,
	/// Series membership at the old path.
	pub series_before: Option<RenameSeries>,
	/// Series membership at the destination path.
	pub series_after: Option<RenameSeries>,
}

/// Plans updating Source Sentry's records after a document is moved.
///
/// Both identities must be local. The old file must be absent, and the new
/// file must be tracked and present. A lockfile declaration moves to the
/// nearest destination lockfile, or a new one beside the destination. Its
/// old tombstones stay in place. Relative sources and reviewed members are
/// re-expressed, retaining every review, seal and completion.
///
/// An on-file declaration must have existed at the old path in `HEAD`, and
/// arrive unchanged in the new file. This proves that the destination's
/// declaration travelled with the file rather than belonging to a different
/// document. No history walk or content-based rename guessing is performed.
///
/// # Errors
///
/// Refuses remote identities, files that have not been moved, an untracked
/// destination, independent destination declarations, ambiguous on-file
/// provenance, sources that cannot be re-expressed, conflicting reviewed
/// members, broken seals, and newly introduced configuration or integrity
/// errors. Fails when the backend cannot read the repository.
pub fn rename<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	configs: &ConfigSet,
	inventory: &Inventory,
	request: &RenameRequest,
) -> Result<Planned<RenameSummary>, MutateError> {
	let (DocumentId::Local(old), DocumentId::Local(new)) =
		(&request.old, &request.new)
	else {
		return Err(refused(
			request,
			"remote identities cannot be renamed; name two filesystem paths",
		));
	};
	let repo = Repo::new(snapshot, inventory)?;
	if repo.tracked.contains(old) {
		return Err(refused(
			request,
			"the old file still exists; move the file first with `git mv <old> <new>`",
		));
	}
	if !repo.tracked.contains(new) {
		return Err(refused(
			request,
			"the new file is absent or not tracked by git; move the file first with `git mv`, or add its path with `git add -N`",
		));
	}
	if is_own_file(new) {
		return Err(MutateError::NotADocument { path: new.clone() });
	}
	let (mut declaration, original, from) = origin(&repo, history, request)?;
	let destination = match &original {
		Placement::OnFile => {
			super::track::on_file_allowed(&repo, configs, new)?;
			Placement::OnFile
		}
		Placement::Lockfile { .. } => Placement::Lockfile {
			directory: repo
				.nearest_lockfile(new)
				.unwrap_or_else(|| new.parent().unwrap_or(RepoPath::ROOT)),
		},
		Placement::Remote { .. } => {
			return Err(refused(
				request,
				"remote identities cannot be renamed; name two filesystem paths",
			));
		}
	};
	let to = location(&repo.inventory.project, &request.new, &destination)?;
	let mut changes = Vec::new();
	sources::rebase(&mut declaration, old, new, &mut changes)?;
	let mut edits = Edits::default();
	if original != Placement::OnFile {
		edits.place(&repo, &request.old, &original, None)?;
	}
	sources::update(&request.new, &mut declaration, old, new, &mut changes)?;
	if original == Placement::OnFile {
		edits.renamed_frontmatter(old, new, declaration);
	} else {
		edits.place(&repo, &request.new, &destination, Some(declaration))?;
	}
	for claim in &inventory.claims {
		if claim.document == request.old
			|| (original == Placement::OnFile && claim.document == request.new)
		{
			continue;
		}
		let mut declaration = claim.declaration.clone();
		if sources::update(
			&claim.document,
			&mut declaration,
			old,
			new,
			&mut changes,
		)? {
			edits.set(&repo, claim, Some(declaration))?;
		}
	}
	let plan = edits.finish(&repo)?;
	let after = verify(&repo, &plan)?;
	let renamed = rediscovered(&after, &request.new, &destination, &to)?;
	validate::integrity(&repo, &after, renamed, new)?;
	Ok(Planned {
		plan,
		summary: RenameSummary {
			old: old.clone(),
			new: new.clone(),
			from,
			to,
			sources: changes,
			series_before: membership(inventory, old, configs),
			series_after: membership(&after, new, configs),
		},
	})
}

/// An actionable refusal tied to both requested identities.
fn refused(request: &RenameRequest, reason: &'static str) -> MutateError {
	MutateError::RenameRefused {
		old: request.old.clone(),
		new: request.new.clone(),
		reason,
	}
}

/// The declaration to carry and its original site. Only missing-document
/// diagnostics are tolerated on a lockfile origin; all other errors must
/// be corrected first.
fn origin<S: Snapshot, H: History>(
	repo: &Repo<'_, S>,
	history: &H,
	request: &RenameRequest,
) -> Result<(Declaration, Placement, Location), MutateError> {
	if repo.inventory.claims_of(&request.old).next().is_some() {
		let claim = repo.claim(&request.old)?;
		repo.refuse_errors(
			&request.old,
			&[
				DiagnosticKind::MissingDocument,
				DiagnosticKind::MissingRecord,
			],
		)?;
		if repo.inventory.claims_of(&request.new).next().is_some() {
			return Err(refused(
				request,
				"the destination already has a declaration; choose an unclaimed destination",
			));
		}
		return Ok((
			claim.declaration.clone(),
			Placement::of(claim),
			claim.location(),
		));
	}
	let DocumentId::Local(old) = &request.old else {
		return Err(refused(request, "remote identities cannot be renamed"));
	};
	let previous = committed_declaration(repo.snapshot, history, old)?;
	let Some(previous) = previous else {
		return Err(refused(
			request,
			"the old path has no live declaration; restore it, or commit its on-file declaration before moving it",
		));
	};
	let DocumentId::Local(new) = &request.new else {
		return Err(refused(request, "remote identities cannot be renamed"));
	};
	let claims: Vec<_> = repo.inventory.claims_of(&request.new).collect();
	if !matches!(
		claims.as_slice(),
		[] | [crate::claims::Claim {
			mode: Mode::OnFile,
			..
		}]
	) {
		return Err(refused(
			request,
			"the destination already has a declaration; choose an unclaimed destination",
		));
	}
	let bytes = repo.snapshot.read(new)?;
	let arrived = frontmatter::read(old, &bytes).map_err(|error| {
		MutateError::InvalidFrontmatter {
			document: new.clone(),
			error,
		}
	})?;
	if arrived.as_ref() != Some(&previous) {
		return Err(refused(
			request,
			"the destination declaration does not match the old file's committed frontmatter; restore the carried declaration before renaming",
		));
	}
	if committed_declaration(repo.snapshot, history, new)?.is_some() {
		return Err(refused(
			request,
			"the destination already had an on-file declaration; choose an unclaimed destination",
		));
	}
	Ok((
		previous,
		Placement::OnFile,
		Location::field(old.clone(), crate::claims::FRONTMATTER_FIELD),
	))
}

/// Reads only the old path at `HEAD`, never guessing from equal content.
fn committed_declaration<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	path: &RepoPath,
) -> Result<Option<Declaration>, MutateError> {
	let Revision::WorkingTree { head: Some(head) } = snapshot.revision() else {
		return Ok(None);
	};
	let Some(oid) = history
		.blobs_at(head, std::slice::from_ref(path))?
		.pop()
		.flatten()
	else {
		return Ok(None);
	};
	let Some(bytes) = history.blob(&oid)? else {
		return Ok(None);
	};
	frontmatter::read(path, &bytes).map_err(|error| {
		MutateError::InvalidFrontmatter {
			document: path.clone(),
			error,
		}
	})
}

/// Membership at a declared or unclaimed path. Config matching covers the
/// old path after an on-file declaration has already travelled away.
fn membership(
	inventory: &Inventory,
	path: &RepoPath,
	configs: &ConfigSet,
) -> Option<RenameSeries> {
	if let Some((series, member)) = inventory.series_of(path) {
		return Some(RenameSeries {
			scope: series.series.scope.clone(),
			name: series.series.name.clone(),
			version: member.version.clone(),
		});
	}
	let settings = configs.settings(&path.parent().unwrap_or(RepoPath::ROOT));
	let found = settings.series_of(path);
	let [(series, version)] = found.as_slice() else {
		return None;
	};
	Some(RenameSeries {
		scope: series.scope.clone(),
		name: series.name.clone(),
		version: Version::parse(series.version_kind, version).ok()?,
	})
}
