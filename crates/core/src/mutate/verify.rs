//! Checking a plan against discovery before it is returned.
//!
//! The plan is applied to an in-memory overlay of the snapshot, the configs
//! are loaded again, because a new or deleted lockfile changes the roots,
//! and discovery
//! runs again. Every configuration or integrity error that the overlay has
//! and the snapshot did not is one the plan would introduce, so the plan is
//! refused.

use std::collections::{BTreeMap, BTreeSet};

use super::MutateError;
use super::repo::Repo;
use crate::claims::{Diagnostic, DiagnosticKind, Inventory, discover};
use crate::config::{ConfigSet, LoadError};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{
	BackendError, BackendErrorKind, Canonical, Revision, Snapshot, WritePlan,
};

/// A snapshot with a plan's files written over it and its deleted files
/// removed.
struct Overlay<'a, S> {
	inner: &'a S,
	files: BTreeMap<&'a RepoPath, &'a [u8]>,
	deleted: BTreeSet<&'a RepoPath>,
}

impl<S> Overlay<'_, S> {
	/// The error for a file the plan deletes.
	fn deleted(path: &RepoPath) -> BackendError {
		BackendError::new(
			BackendErrorKind::Io,
			format!("{path} is deleted by the plan"),
		)
	}
}

impl<S: Snapshot> Snapshot for Overlay<'_, S> {
	fn revision(&self) -> &Revision {
		self.inner.revision()
	}

	fn paths(&self) -> Result<Vec<RepoPath>, BackendError> {
		let mut paths: BTreeSet<RepoPath> =
			self.inner.paths()?.into_iter().collect();
		paths.extend(self.files.keys().map(|path| (*path).clone()));
		paths.retain(|path| !self.deleted.contains(path));
		Ok(paths.into_iter().collect())
	}

	fn read(&self, path: &RepoPath) -> Result<Vec<u8>, BackendError> {
		if self.deleted.contains(path) {
			return Err(Self::deleted(path));
		}
		match self.files.get(path) {
			Some(contents) => Ok(contents.to_vec()),
			None => self.inner.read(path),
		}
	}

	/// Planned files have no canonical oid until the backend writes them,
	/// and deleted files have none at all. Discovery and config loading never
	/// ask for one.
	fn canonical_oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, BackendError> {
		if let Some(path) = paths.iter().find(|path| self.files.contains_key(path))
		{
			return Err(BackendError::new(
				BackendErrorKind::Io,
				format!("{path} is planned, and has no canonical oid until written"),
			));
		}
		if let Some(path) = paths.iter().find(|path| self.deleted.contains(path)) {
			return Err(Self::deleted(path));
		}
		self.inner.canonical_oids(paths)
	}
}

/// Discovers the repository as it would be once `plan` is applied.
///
/// `repo.inventory` must have been discovered from `repo.snapshot`.
///
/// # Errors
///
/// Fails with [`MutateError::WouldIntroduce`] when the plan would introduce
/// a configuration or integrity error, with [`MutateError::InvalidConfigs`]
/// when the configs would no longer load, and when the backend fails.
pub(super) fn verify<S: Snapshot>(
	repo: &Repo<'_, S>,
	plan: &WritePlan,
) -> Result<Inventory, MutateError> {
	let overlay = Overlay {
		inner: repo.snapshot,
		files: plan
			.writes
			.iter()
			.map(|write| (&write.path, write.contents.as_slice()))
			.collect(),
		deleted: plan.deletes.iter().map(|delete| &delete.path).collect(),
	};
	let configs = ConfigSet::load(&overlay).map_err(|error| match error {
		LoadError::Backend(error) => MutateError::Backend(error),
		LoadError::Invalid(errors) => MutateError::InvalidConfigs { errors },
	})?;
	let after = discover(&overlay, &configs)?;
	let introduced = introduced(repo.inventory, &after);
	if introduced.is_empty() {
		Ok(after)
	} else {
		Err(MutateError::WouldIntroduce {
			diagnostics: introduced,
		})
	}
}

/// What identifies a diagnostic across a change that moves declarations: its
/// kind and the document it concerns, or, without a document, its kind and
/// file.
type Key = (DiagnosticKind, Option<DocumentId>, Option<RepoPath>);

fn key(diagnostic: &Diagnostic) -> Key {
	match &diagnostic.document {
		Some(document) => (diagnostic.kind, Some(document.clone()), None),
		None => (
			diagnostic.kind,
			None,
			Some(diagnostic.location.file.clone()),
		),
	}
}

/// The fatal diagnostics of `after` that `before` does not have, counting
/// repeats.
fn introduced(before: &Inventory, after: &Inventory) -> Vec<Diagnostic> {
	let mut known: BTreeMap<Key, usize> = BTreeMap::new();
	for diagnostic in &before.diagnostics {
		if diagnostic.severity().is_fatal() {
			*known.entry(key(diagnostic)).or_default() += 1;
		}
	}
	let mut introduced = Vec::new();
	for diagnostic in &after.diagnostics {
		if !diagnostic.severity().is_fatal() {
			continue;
		}
		match known.get_mut(&key(diagnostic)) {
			Some(count) if *count > 0 => *count -= 1,
			_ => introduced.push(diagnostic.clone()),
		}
	}
	introduced
}
