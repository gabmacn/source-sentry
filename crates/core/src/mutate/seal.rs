//! `seal`: making a lockfile-tracked document a record.

use chrono::{DateTime, SubsecRound, Utc};

use super::edits::Edits;
use super::repo::{Repo, checked_reviewer};
use super::verify::verify;
use super::{MutateError, Planned};
use crate::claims::{Inventory, Mode};
use crate::model::{DocumentId, RepoPath, Seal};
use crate::ports::Snapshot;

/// What to seal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealRequest {
	/// The document, which must be a local lockfile entry.
	pub document: DocumentId,
	/// Who seals it, which the CLI passes in as it does for `attest`.
	pub by: Option<String>,
	/// When, which is truncated to the second.
	pub at: DateTime<Utc>,
}

/// What a seal records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealSummary {
	/// The record.
	pub document: RepoPath,
	/// The lockfile that lists it.
	pub lockfile: RepoPath,
	/// The seal, whose oid is the record's canonical blob oid.
	pub seal: Seal,
}

/// Plans sealing a document: its entry gains `seal: {at, by, oid}`, where
/// `oid` is the document's own canonical blob oid.
/// Its sources are frozen from then on.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - a remote document, since remote documents cannot be records
///   ([`MutateError::RemoteRecord`]);
/// - an untracked or doubly claimed document, or one with configuration or
///   integrity errors;
/// - an on-file document ([`MutateError::OnFileRecord`]): records are
///   lockfile-only, so it must move with `track --from` first;
/// - a document that is already sealed ([`MutateError::AlreadySealed`]),
///   since a correction is a new record;
/// - the living head of a `living` series ([`MutateError::SealLivingHead`]);
/// - a document without a canonical oid, and an invalid sealer's name.
///
/// Fails when the backend fails.
pub fn seal<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
	request: &SealRequest,
) -> Result<Planned<SealSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let by = checked_reviewer(request.by.as_deref())?;
	let DocumentId::Local(path) = &request.document else {
		return Err(MutateError::RemoteRecord {
			document: request.document.clone(),
		});
	};
	let claim = repo.claim(&request.document)?;
	match claim.mode {
		Mode::LocalLockfile => {}
		Mode::OnFile => {
			return Err(MutateError::OnFileRecord {
				document: claim.document.clone(),
			});
		}
		Mode::RemoteLockfile { .. } => {
			return Err(MutateError::RemoteRecord {
				document: claim.document.clone(),
			});
		}
	}
	if let Some(seal) = &claim.declaration.seal {
		return Err(MutateError::AlreadySealed {
			document: claim.document.clone(),
			at: seal.at,
		});
	}
	repo.refuse_errors(&claim.document, &[])?;
	if let Some(series) = repo.living_head(path) {
		return Err(MutateError::SealLivingHead {
			document: claim.document.clone(),
			series: series.series.name.clone(),
		});
	}
	let seal = Seal {
		at: request.at.trunc_subsecs(0),
		by,
		oid: repo.oid(path)?,
	};
	let mut declaration = claim.declaration.clone();
	declaration.seal = Some(seal.clone());

	let mut edits = Edits::default();
	edits.set(&repo, claim, Some(declaration))?;
	let plan = edits.finish(&repo)?;
	verify(&repo, &plan)?;
	Ok(Planned {
		plan,
		summary: SealSummary {
			document: path.clone(),
			lockfile: claim.declared_in.clone(),
			seal,
		},
	})
}
