//! What every command reads: the snapshot's paths, the one claim of a
//! document, the errors discovery reports about it, canonical oids, and
//! where a declaration is placed.

use std::collections::BTreeSet;
use std::fmt;

use super::MutateError;
use crate::claims::{
	Claim, DiagnosticKind, Inventory, LockfileKind, Mode, SeriesMembers,
};
use crate::model::{DocumentId, Lifecycle, ObjectId, RepoPath};
use crate::ports::{BackendError, BackendErrorKind, Canonical, Snapshot};
use crate::{DIR_NAME, LOCKFILE_NAME, REMOTES_DIR_NAME};

/// Where a document's declaration lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Placement {
	/// In the document's own frontmatter.
	OnFile,
	/// In the local lockfile `<directory>/source-sentry.lock`.
	Lockfile {
		/// The directory that holds the lockfile.
		directory: RepoPath,
	},
	/// In the remote owner lockfile `.source-sentry/remotes/<owner>.lock`.
	Remote {
		/// The owner, which names the lockfile and its paired config.
		owner: String,
	},
}

impl Placement {
	/// Where `claim`'s declaration lives.
	#[must_use]
	pub fn of(claim: &Claim) -> Self {
		match &claim.mode {
			Mode::OnFile => Self::OnFile,
			Mode::LocalLockfile => Self::Lockfile {
				directory: claim.declared_in.parent().unwrap_or(RepoPath::ROOT),
			},
			Mode::RemoteLockfile { owner } => Self::Remote {
				owner: owner.clone(),
			},
		}
	}
}

impl fmt::Display for Placement {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::OnFile => formatter.write_str("on-file"),
			Self::Lockfile { directory } if directory.is_root() => {
				write!(formatter, "in {LOCKFILE_NAME}")
			}
			Self::Lockfile { directory } => {
				write!(formatter, "in {directory}/{LOCKFILE_NAME}")
			}
			Self::Remote { owner } => {
				write!(formatter, "in the owner lockfile of `{owner}`")
			}
		}
	}
}

/// The local lockfile of `directory`.
pub(super) fn lockfile_path(directory: &RepoPath) -> RepoPath {
	#[expect(
		clippy::expect_used,
		reason = "a canonical directory joined with a constant file name is \
		          always a valid path"
	)]
	directory
		.join(LOCKFILE_NAME)
		.expect("a directory and a file name form a path")
}

/// The owner lockfile of `owner`, after checking that the name can be a
/// file name of its own.
///
/// # Errors
///
/// Fails with [`MutateError::InvalidOwnerName`] unless `owner` is made of
/// ASCII letters, digits, `-`, `_` and `.`, is not empty, and does not start
/// with `.`.
pub(super) fn remote_lockfile_path(
	owner: &str,
) -> Result<RepoPath, MutateError> {
	let valid = !owner.is_empty()
		&& !owner.starts_with('.')
		&& owner
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
	let invalid = || MutateError::InvalidOwnerName {
		owner: owner.to_owned(),
	};
	if !valid {
		return Err(invalid());
	}
	RepoPath::new(&format!("{DIR_NAME}/{REMOTES_DIR_NAME}/{owner}.lock"))
		.map_err(|_| invalid())
}

/// Whether `path` is one of Source Sentry's own files: anything under
/// `.source-sentry/`, a lockfile or a directory config.
pub(super) fn is_own_file(path: &RepoPath) -> bool {
	path.components().next() == Some(DIR_NAME)
		|| matches!(
			path.file_name(),
			Some(name) if name == LOCKFILE_NAME || name == crate::CONFIG_NAME
		)
}

/// Checks a reviewer's name before it is written: names only,
/// never emails, and never blank.
///
/// # Errors
///
/// Fails with [`MutateError::InvalidReviewer`] when the name is blank,
/// contains a control character, or contains `@`, which would make it an
/// email address or part of one.
pub(super) fn checked_reviewer(
	reviewer: Option<&str>,
) -> Result<Option<String>, MutateError> {
	let Some(name) = reviewer else {
		return Ok(None);
	};
	let reason = if name.trim().is_empty() {
		Some("it is blank; omit it to record an unknown reviewer")
	} else if name.chars().any(char::is_control) {
		Some("it contains a control character")
	} else if name.contains('@') {
		Some("only names are recorded, never email addresses")
	} else {
		None
	};
	match reason {
		Some(reason) => Err(MutateError::InvalidReviewer {
			reviewer: name.to_owned(),
			reason,
		}),
		None => Ok(Some(name.to_owned())),
	}
}

/// The snapshot and its inventory, as one command reads them.
pub(super) struct Repo<'a, S> {
	/// The snapshot that the inventory was discovered from.
	pub(super) snapshot: &'a S,
	/// The inventory.
	pub(super) inventory: &'a Inventory,
	/// Every tracked path of the snapshot.
	pub(super) tracked: BTreeSet<RepoPath>,
}

impl<'a, S: Snapshot> Repo<'a, S> {
	/// Reads the snapshot's paths.
	///
	/// # Errors
	///
	/// Fails when the backend cannot list them.
	pub(super) fn new(
		snapshot: &'a S,
		inventory: &'a Inventory,
	) -> Result<Self, MutateError> {
		Ok(Self {
			snapshot,
			inventory,
			tracked: snapshot.paths()?.into_iter().collect(),
		})
	}

	/// The one claim of `document`.
	///
	/// # Errors
	///
	/// Fails with [`MutateError::NotTracked`] when nothing claims it, and
	/// with [`MutateError::DoubleClaim`] when more than one declaration does.
	pub(super) fn claim(
		&self,
		document: &DocumentId,
	) -> Result<&'a Claim, MutateError> {
		let inventory = self.inventory;
		let claims: Vec<&'a Claim> = inventory
			.claims
			.iter()
			.filter(|claim| &claim.document == document)
			.collect();
		match claims.as_slice() {
			[claim] => Ok(claim),
			// A declaration that cannot be read makes no claim; its errors,
			// not `track`, are what to fix.
			[] => Err(self.refuse_errors(document, &[]).err().unwrap_or_else(|| {
				MutateError::NotTracked {
					document: document.clone(),
					retired: inventory
						.tombstones
						.iter()
						.any(|tombstone| &tombstone.document == document),
				}
			})),
			_ => Err(MutateError::DoubleClaim {
				document: document.clone(),
				sites: claims.iter().map(|claim| claim.location()).collect(),
			}),
		}
	}

	/// Refuses when discovery reports a configuration or integrity error
	/// about `document`, other than the kinds in `tolerated`. Warnings never
	/// refuse.
	///
	/// # Errors
	///
	/// Fails with [`MutateError::DocumentHasErrors`], listing the errors.
	pub(super) fn refuse_errors(
		&self,
		document: &DocumentId,
		tolerated: &[DiagnosticKind],
	) -> Result<(), MutateError> {
		let diagnostics: Vec<_> = self
			.inventory
			.diagnostics
			.iter()
			.filter(|diagnostic| {
				diagnostic.severity().is_fatal()
					&& diagnostic.document.as_ref() == Some(document)
					&& !tolerated.contains(&diagnostic.kind)
			})
			.cloned()
			.collect();
		if diagnostics.is_empty() {
			Ok(())
		} else {
			Err(MutateError::DocumentHasErrors {
				document: document.clone(),
				diagnostics,
			})
		}
	}

	/// The canonical values of `paths`, in order, which must all be tracked.
	///
	/// # Errors
	///
	/// Fails when the backend fails, or returns a different number of values
	/// than paths.
	pub(super) fn canonical(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, MutateError> {
		if paths.is_empty() {
			return Ok(Vec::new());
		}
		let values = self.snapshot.canonical_oids(paths)?;
		if values.len() != paths.len() {
			return Err(MutateError::Backend(BackendError::new(
				BackendErrorKind::Git,
				format!(
					"the backend returned {} canonical oids for {} paths",
					values.len(),
					paths.len()
				),
			)));
		}
		Ok(values)
	}

	/// The canonical oids of `paths`, in order, which must all be tracked.
	///
	/// # Errors
	///
	/// Fails as [`Repo::canonical`] does, and with
	/// [`MutateError::NotCanonical`] for the first path whose canonical value
	/// is not an oid.
	pub(super) fn oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<ObjectId>, MutateError> {
		paths
			.iter()
			.zip(self.canonical(paths)?)
			.map(|(path, canonical)| match canonical {
				Canonical::Oid(oid) => Ok(oid),
				canonical => Err(MutateError::NotCanonical {
					path: path.clone(),
					canonical,
				}),
			})
			.collect()
	}

	/// The canonical oid of one tracked path.
	///
	/// # Errors
	///
	/// Fails as [`Repo::oids`] does.
	pub(super) fn oid(&self, path: &RepoPath) -> Result<ObjectId, MutateError> {
		self.oids(std::slice::from_ref(path))?.pop().ok_or_else(|| {
			MutateError::Backend(BackendError::new(
				BackendErrorKind::Git,
				format!("the backend returned no canonical oid for {path}"),
			))
		})
	}

	/// The `living` series whose newest entry is `path`, if any.
	pub(super) fn living_head(
		&self,
		path: &RepoPath,
	) -> Option<&'a SeriesMembers> {
		let inventory = self.inventory;
		inventory.series.iter().find(|series| {
			series.series.lifecycle == Lifecycle::Living
				&& series.members.last().is_some_and(|head| &head.path == path)
		})
	}

	/// The directory of the nearest local lockfile whose authority covers
	/// `path`.
	pub(super) fn nearest_lockfile(&self, path: &RepoPath) -> Option<RepoPath> {
		self.inventory.lockfile_for(path).and_then(|lockfile| {
			match &lockfile.kind {
				LockfileKind::Local { directory } => Some(directory.clone()),
				LockfileKind::Remote { .. } => None,
			}
		})
	}

	/// The directories of the local lockfiles strictly between `document`
	/// and `directory`: inside `directory`, and holding `document` in their
	/// subtree. The nearest to `document` comes first. A lockfile that failed
	/// to parse is included.
	pub(super) fn lockfiles_between(
		&self,
		document: &RepoPath,
		directory: &RepoPath,
	) -> Vec<RepoPath> {
		let mut between: Vec<RepoPath> = self
			.inventory
			.lockfiles
			.iter()
			.filter_map(|lockfile| match &lockfile.kind {
				LockfileKind::Local { directory: inner }
					if inner != directory
						&& inner.starts_with(directory)
						&& inner != document
						&& document.starts_with(inner) =>
				{
					Some(inner.clone())
				}
				LockfileKind::Local { .. } | LockfileKind::Remote { .. } => None,
			})
			.collect();
		between.sort_by_key(|inner| std::cmp::Reverse(inner.components().count()));
		between
	}

	/// The documents that the local lockfile at `lockfile` lists, sorted.
	pub(super) fn entries_of(&self, lockfile: &RepoPath) -> Vec<DocumentId> {
		self
			.inventory
			.claims
			.iter()
			.filter(|claim| {
				claim.mode == Mode::LocalLockfile && &claim.declared_in == lockfile
			})
			.map(|claim| claim.document.clone())
			.collect()
	}

	/// The documents whose live sources include `path`: each path source and
	/// each current pattern match, sealed records excepted.
	pub(super) fn dependents_of(&self, path: &RepoPath) -> Vec<DocumentId> {
		let mut dependents: Vec<DocumentId> = self
			.inventory
			.claims
			.iter()
			.filter(|claim| {
				!claim.is_sealed()
					&& claim.document != DocumentId::Local(path.clone())
					&& claim.source_paths().contains(path)
			})
			.map(|claim| claim.document.clone())
			.collect();
		dependents.sort();
		dependents.dedup();
		dependents
	}
}
