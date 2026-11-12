//! `retire`: replacing a lockfile entry with a tombstone.

use chrono::{DateTime, Utc};

use super::edits::Edits;
use super::repo::{
	Placement, Repo, checked_reviewer, lockfile_path, remote_lockfile_path,
};
use super::verify::verify;
use super::{MutateError, Planned};
use crate::claims::{DiagnosticKind, Inventory};
use crate::model::{DocumentId, RepoPath, Retired, Review};
use crate::ports::Snapshot;

/// What to retire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetireRequest {
	/// The document: a local or a remote lockfile entry.
	pub document: DocumentId,
	/// Who retires it, which the CLI passes in as it does for `attest`.
	pub by: Option<String>,
	/// When, which is truncated to the second.
	pub at: DateTime<Utc>,
}

/// What retiring records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetireSummary {
	/// The retired document.
	pub document: DocumentId,
	/// The lockfile that holds the tombstone.
	pub lockfile: RepoPath,
	/// The tombstone, which keeps the seal of a record.
	pub tombstone: Retired,
}

/// The errors about the document that retiring resolves, because it removes
/// the entry that has them: a listed document that is missing (the remedy
/// that discovery suggests), and problems with its sources and classes.
const TOLERATED: &[DiagnosticKind] = &[
	DiagnosticKind::MissingDocument,
	DiagnosticKind::MissingRecord,
	DiagnosticKind::MissingSource,
	DiagnosticKind::InvalidSourcePath,
	DiagnosticKind::InvalidPattern,
	DiagnosticKind::SelfDependency,
	DiagnosticKind::UndeclaredClass,
	DiagnosticKind::DependencyCycle,
];

/// Plans retiring a document: its lockfile entry moves from `documents` to
/// `retired`, keeping its seal and completion alongside earlier tombstones.
/// Local and remote lockfiles work alike. The document itself is not touched;
/// deleting it is a separate change.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - an untracked or doubly claimed document;
/// - an on-file document ([`MutateError::OnFileRetire`]), which needs no
///   tombstone, because deleting the file removes its claim;
/// - a document with configuration or integrity errors other than those
///   retiring resolves: being missing, and problems with its sources or
///   classes;
/// - the living head of a `living` series ([`MutateError::RetireLivingHead`]),
///   naming the config that defines the series;
/// - an invalid name for who retires it.
///
/// Fails when the backend fails.
pub fn retire<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
	request: &RetireRequest,
) -> Result<Planned<RetireSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let by = checked_reviewer(request.by.as_deref())?;
	let claim = repo.claim(&request.document)?;
	let placement = Placement::of(claim);
	if placement == Placement::OnFile {
		return Err(MutateError::OnFileRetire {
			document: claim.document.clone(),
		});
	}
	repo.refuse_errors(&claim.document, TOLERATED)?;
	if let DocumentId::Local(path) = &claim.document
		&& let Some(series) = repo.living_head(path)
	{
		return Err(MutateError::RetireLivingHead {
			document: claim.document.clone(),
			series: series.series.name.clone(),
			config: series.series.origin.file.clone(),
		});
	}
	let tombstone = Retired {
		retired: Review::new(request.at, by),
		seal: claim.declaration.seal.clone(),
		completed: claim.declaration.completed.clone(),
	};

	let mut edits = Edits::default();
	let lockfile = match (&claim.document, &placement) {
		(DocumentId::Local(path), Placement::Lockfile { directory }) => {
			let lockfile = edits.local(&repo, directory)?;
			lockfile.documents.remove(path);
			insert_tombstone(
				lockfile.retired.entry(path.clone()).or_default(),
				tombstone.clone(),
			);
			lockfile_path(directory)
		}
		(DocumentId::Remote(identity), Placement::Remote { owner }) => {
			let lockfile = edits.remote(&repo, owner)?;
			lockfile.documents.remove(identity);
			insert_tombstone(
				lockfile.retired.entry(identity.clone()).or_default(),
				tombstone.clone(),
			);
			remote_lockfile_path(owner)?
		}
		(document, placement) => {
			return Err(MutateError::TargetMismatch {
				document: document.clone(),
				target: placement.to_string(),
			});
		}
	};
	let plan = edits.finish(&repo)?;
	verify(&repo, &plan)?;
	Ok(Planned {
		plan,
		summary: RetireSummary {
			document: claim.document.clone(),
			lockfile,
			tombstone,
		},
	})
}

/// Adds a tombstone in retirement order, after existing equal instants.
/// A clock that moves backwards cannot make the written list invalid.
fn insert_tombstone(tombstones: &mut Vec<Retired>, tombstone: Retired) {
	let index = tombstones
		.partition_point(|previous| previous.retired.at <= tombstone.retired.at);
	tombstones.insert(index, tombstone);
}
