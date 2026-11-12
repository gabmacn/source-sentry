//! `diff`: what changed in a document's sources since they were attested,
//! and the expectations to pass to `attest`.
//!
//! `diff` returns data only. Rendering text diffs belongs to the CLI.

use std::collections::BTreeSet;

use super::expectation::Expectation;
use super::repo::Repo;
use super::select::{SourceState, observe};
use super::{MutateError, Selection};
use crate::claims::{Attestation, DiagnosticKind, Inventory};
use crate::model::{DocumentId, ObjectId, RepoPath, Review, SourceDecl};
use crate::ports::{History, Snapshot};

/// A document's selected sources, each compared with its attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentDiff {
	/// The document.
	pub document: DocumentId,
	/// Each selected source, in declared order.
	pub sources: Vec<SourceDiff>,
	/// The expectations to pass to `attest` after review: every file of the
	/// selection and every removal from a pattern, sorted by path.
	pub expectations: Vec<Expectation>,
}

/// One source's files, compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDiff {
	/// The path or pattern, as declared.
	pub declared: String,
	/// Whether it is a pattern.
	pub pattern: bool,
	/// The path of a path source, or every current and former member of a
	/// pattern, sorted by path.
	pub entries: Vec<EntryDiff>,
	/// The source's own last review, independent of its files' baselines.
	/// A reviewed empty pattern can be attested again.
	pub reviewed: Option<Review>,
}

/// One file, as attested and as it is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryDiff {
	/// The file.
	pub path: RepoPath,
	/// What was attested.
	pub baseline: Baseline,
	/// What the snapshot holds.
	pub current: Current,
}

impl EntryDiff {
	/// Whether the file differs from what was attested: changed, added,
	/// removed, or never attested.
	#[must_use]
	pub fn is_changed(&self) -> bool {
		let attested = match &self.baseline {
			Baseline::Unattested => None,
			Baseline::Unavailable { oid } | Baseline::Available { oid, .. } => {
				Some(oid)
			}
		};
		let current = match &self.current {
			Current::Missing => None,
			Current::Present { oid, .. } => Some(oid),
		};
		attested.is_none() || attested != current
	}
}

/// What was attested for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
	/// Nothing: the source was never attested, or the file is a new match
	/// of a pattern.
	Unattested,
	/// The attested blob is not in the object database, so its content
	/// cannot be shown: "baseline unavailable".
	Unavailable {
		/// The attested oid.
		oid: ObjectId,
	},
	/// The attested blob.
	Available {
		/// The attested oid.
		oid: ObjectId,
		/// Its content.
		content: Vec<u8>,
	},
}

/// What the snapshot holds for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Current {
	/// The file has left its pattern, usually because it was deleted.
	Missing,
	/// The file, as the snapshot shows it.
	Present {
		/// Its canonical oid.
		oid: ObjectId,
		/// Its content as its canonical blob holds it, or `None` when that
		/// cannot be had (see [`Snapshot::read_canonical`]).
		content: Option<Vec<u8>>,
	},
}

/// The errors about the document that `diff` handles itself: a missing
/// source refuses only when it is selected.
const TOLERATED: &[DiagnosticKind] = &[DiagnosticKind::MissingSource];

/// Compares the selected sources of `document` with their attestations,
/// reading baselines from `history`.
///
/// The returned expectations are exactly those `attest` compares for the
/// same selection, so passing them to `attest` binds the attestation to
/// what was reviewed.
///
/// # Errors
///
/// Refuses, as `attest` does, when the document is untracked, claimed
/// twice, has configuration or integrity errors other than a missing source
/// that is not selected, or is a sealed record, whose sources are frozen
/// ([`MutateError::Sealed`]); when the selection names no source or an
/// unknown one, or includes a missing path source; and for a file without a
/// canonical oid. Fails when the backend fails.
pub fn diff<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	inventory: &Inventory,
	document: &DocumentId,
	selection: &Selection,
) -> Result<DocumentDiff, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let claim = repo.claim(document)?;
	if claim.is_sealed() {
		return Err(MutateError::Sealed {
			document: document.clone(),
		});
	}
	repo.refuse_errors(document, TOLERATED)?;
	let observed = observe(&repo, claim, selection)?;
	let mut sources = Vec::new();
	for source in &observed.sources {
		let (pattern, entries) = match &source.state {
			SourceState::Path {
				path,
				attestation,
				current,
			} => (
				false,
				vec![compare(
					snapshot,
					history,
					path,
					attestation.as_ref(),
					Some(current),
				)?],
			),
			SourceState::Pattern { matches, members } => {
				let paths: BTreeSet<&RepoPath> =
					matches.keys().chain(members.keys()).collect();
				let entries = paths
					.into_iter()
					.map(|path| {
						compare(
							snapshot,
							history,
							path,
							members.get(path),
							matches.get(path),
						)
					})
					.collect::<Result<_, _>>()?;
				(true, entries)
			}
		};
		sources.push(SourceDiff {
			declared: source.declared.clone(),
			pattern,
			entries,
			reviewed: claim.declaration.sources.get(source.index).and_then(
				|source| match source {
					SourceDecl::Path(source) => source.reviewed.clone(),
					SourceDecl::Pattern(source) => source.reviewed.clone(),
				},
			),
		});
	}
	Ok(DocumentDiff {
		document: document.clone(),
		sources,
		expectations: observed.expectations(),
	})
}

/// One file's baseline and current state.
fn compare<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	path: &RepoPath,
	attestation: Option<&Attestation>,
	current: Option<&ObjectId>,
) -> Result<EntryDiff, MutateError> {
	let baseline = match attestation {
		None => Baseline::Unattested,
		Some(attestation) => match history.blob(&attestation.oid)? {
			Some(content) => Baseline::Available {
				oid: attestation.oid.clone(),
				content,
			},
			None => Baseline::Unavailable {
				oid: attestation.oid.clone(),
			},
		},
	};
	let current = match current {
		None => Current::Missing,
		Some(oid) => Current::Present {
			oid: oid.clone(),
			content: snapshot.read_canonical(path, oid)?,
		},
	};
	Ok(EntryDiff {
		path: path.clone(),
		baseline,
		current,
	})
}
