//! Collecting a command's changes to lockfiles and frontmatter, and turning
//! them into a [`WritePlan`].
//!
//! A command loads each file it changes once, edits it in memory, and calls
//! [`Edits::finish`], which writes every changed file through its adapter,
//! parses the bytes back, deletes the lockfiles the change empties, and
//! records each file's current canonical oid.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use super::MutateError;
use super::repo::{Placement, Repo, lockfile_path, remote_lockfile_path};
use crate::adapters::frontmatter;
use crate::adapters::lockfile::{LocalLockfile, RemoteLockfile};
use crate::claims::Claim;
use crate::model::{Declaration, DocumentId, ObjectId, RepoPath};
use crate::ports::{PlannedDelete, PlannedWrite, Snapshot, WritePlan};

/// A lockfile held in memory, as the snapshot holds it and as edited.
#[derive(Debug)]
struct Loaded<T> {
	/// The lockfile as the snapshot holds it, or `None` when it does not
	/// exist yet.
	original: Option<T>,
	/// The lockfile as edited.
	current: T,
}

/// What [`Edits::finish`] does with one lockfile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
	/// Nothing: the lockfile is unchanged, or it is new and empty.
	Keep,
	/// Write it.
	Write,
	/// Delete it, because the change left it with no documents and no
	/// tombstones.
	Delete,
}

impl<T: PartialEq> Loaded<T> {
	/// What becomes of the lockfile, given whether it is now `empty`.
	fn fate(&self, empty: bool) -> Fate {
		match &self.original {
			Some(original) if *original == self.current => Fate::Keep,
			Some(_) if empty => Fate::Delete,
			None if empty => Fate::Keep,
			Some(_) | None => Fate::Write,
		}
	}
}

/// The files one command changes, held in memory until
/// [`Edits::finish`].
#[derive(Debug, Default)]
pub(super) struct Edits {
	/// Local lockfiles, by directory.
	locals: BTreeMap<RepoPath, Loaded<LocalLockfile>>,
	/// Remote owner lockfiles, by owner.
	remotes: BTreeMap<String, Loaded<RemoteLockfile>>,
	/// On-file declarations, by document: the new declaration, or `None`
	/// to remove the `source-sentry` key.
	documents: BTreeMap<RepoPath, Option<Declaration>>,
	/// Former paths of carried frontmatter, whose old spellings must be
	/// validated at their original location before replacement.
	former_documents: BTreeMap<RepoPath, RepoPath>,
}

impl Edits {
	/// The local lockfile of `directory`: as the snapshot holds it, or empty
	/// when it does not exist yet.
	///
	/// # Errors
	///
	/// Fails when the snapshot cannot read the lockfile, and with
	/// [`MutateError::InvalidLockfile`] when it does not parse, since a
	/// lockfile that cannot be read is never rewritten.
	pub(super) fn local<S: Snapshot>(
		&mut self,
		repo: &Repo<'_, S>,
		directory: &RepoPath,
	) -> Result<&mut LocalLockfile, MutateError> {
		match self.locals.entry(directory.clone()) {
			Entry::Occupied(entry) => Ok(&mut entry.into_mut().current),
			Entry::Vacant(entry) => {
				let path = lockfile_path(directory);
				let original = if repo.tracked.contains(&path) {
					let bytes = repo.snapshot.read(&path)?;
					Some(
						LocalLockfile::parse_in_project(
							&repo.inventory.project,
							directory,
							&bytes,
						)
						.map_err(|error| MutateError::InvalidLockfile { path, error })?,
					)
				} else {
					None
				};
				let current = original
					.clone()
					.unwrap_or_else(|| LocalLockfile::new(directory.clone()));
				Ok(&mut entry.insert(Loaded { original, current }).current)
			}
		}
	}

	/// The owner lockfile of `owner`: as the snapshot holds it, or empty when
	/// it does not exist yet.
	///
	/// # Errors
	///
	/// Fails as [`Edits::local`] does, and with
	/// [`MutateError::InvalidOwnerName`] when `owner` cannot name a file.
	pub(super) fn remote<S: Snapshot>(
		&mut self,
		repo: &Repo<'_, S>,
		owner: &str,
	) -> Result<&mut RemoteLockfile, MutateError> {
		match self.remotes.entry(owner.to_owned()) {
			Entry::Occupied(entry) => Ok(&mut entry.into_mut().current),
			Entry::Vacant(entry) => {
				let path = remote_lockfile_path(&repo.inventory.project, owner)?;
				let original = if repo.tracked.contains(&path) {
					let bytes = repo.snapshot.read(&path)?;
					Some(
						RemoteLockfile::parse_in_project(&repo.inventory.project, &bytes)
							.map_err(|error| MutateError::InvalidLockfile { path, error })?,
					)
				} else {
					None
				};
				let current = original.clone().unwrap_or_default();
				Ok(&mut entry.insert(Loaded { original, current }).current)
			}
		}
	}

	/// Sets the on-file declaration of `document`, or removes it with
	/// `None`.
	pub(super) fn frontmatter(
		&mut self,
		document: &RepoPath,
		declaration: Option<Declaration>,
	) {
		self.documents.insert(document.clone(), declaration);
	}

	/// Rewrites frontmatter that travelled from `old` to `new`, validating
	/// the existing source spellings against their original directory.
	pub(super) fn renamed_frontmatter(
		&mut self,
		old: &RepoPath,
		new: &RepoPath,
		declaration: Declaration,
	) {
		self.former_documents.insert(new.clone(), old.clone());
		self.frontmatter(new, Some(declaration));
	}

	/// Sets the declaration of `document` at `placement`, or removes it with
	/// `None`. A lockfile entry that is removed leaves no tombstone.
	///
	/// # Errors
	///
	/// Fails as [`Edits::local`] and [`Edits::remote`] do, and with
	/// [`MutateError::TargetMismatch`] when the placement does not fit the
	/// document's kind.
	pub(super) fn place<S: Snapshot>(
		&mut self,
		repo: &Repo<'_, S>,
		document: &DocumentId,
		placement: &Placement,
		declaration: Option<Declaration>,
	) -> Result<(), MutateError> {
		match (document, placement) {
			(DocumentId::Local(path), Placement::OnFile) => {
				self.frontmatter(path, declaration);
			}
			(DocumentId::Local(path), Placement::Lockfile { directory }) => {
				let documents = &mut self.local(repo, directory)?.documents;
				match declaration {
					Some(declaration) => {
						documents.insert(path.clone(), declaration);
					}
					None => {
						documents.remove(path);
					}
				}
			}
			(DocumentId::Remote(identity), Placement::Remote { owner }) => {
				let documents = &mut self.remote(repo, owner)?.documents;
				match declaration {
					Some(declaration) => {
						documents.insert(identity.clone(), declaration);
					}
					None => {
						documents.remove(identity);
					}
				}
			}
			(document, placement) => {
				return Err(MutateError::TargetMismatch {
					document: document.clone(),
					target: placement.to_string(),
				});
			}
		}
		Ok(())
	}

	/// Replaces `claim`'s declaration where it lives, or removes it with
	/// `None`.
	///
	/// # Errors
	///
	/// Fails as [`Edits::place`] does.
	pub(super) fn set<S: Snapshot>(
		&mut self,
		repo: &Repo<'_, S>,
		claim: &Claim,
		declaration: Option<Declaration>,
	) -> Result<(), MutateError> {
		self.place(repo, &claim.document, &Placement::of(claim), declaration)
	}

	/// Writes every changed file and returns the plan, sorted by path.
	///
	/// - A changed lockfile is written with its adapter's `to_bytes` and
	///   parsed back. One that the change leaves with no documents and no
	///   tombstones is deleted instead, and a new one that would be empty is
	///   never created.
	/// - A document's frontmatter is rewritten with [`frontmatter::write`] and
	///   must read back as the planned declaration.
	/// - A file whose contents would not change is left out.
	///
	/// Each written file that the snapshot tracks expects its current
	/// canonical oid, and a new file expects to be absent. Each deleted file
	/// expects its current canonical oid.
	///
	/// # Errors
	///
	/// Fails with [`MutateError::InvalidLockfile`] or
	/// [`MutateError::InvalidFrontmatter`] when a file cannot be written, with
	/// [`MutateError::RoundTrip`] when it would not read back as planned, with
	/// [`MutateError::NotCanonical`] when a changed or deleted file has no
	/// canonical oid, and when the backend fails.
	pub(super) fn finish<S: Snapshot>(
		self,
		repo: &Repo<'_, S>,
	) -> Result<WritePlan, MutateError> {
		let mut files: BTreeMap<RepoPath, Vec<u8>> = BTreeMap::new();
		let mut deleted: Vec<RepoPath> = Vec::new();
		for (directory, loaded) in self.locals {
			let path = lockfile_path(&directory);
			let lockfile = &loaded.current;
			let empty = lockfile.documents.is_empty() && lockfile.retired.is_empty();
			match loaded.fate(empty) {
				Fate::Keep => {}
				Fate::Write => {
					let bytes = write_local(&repo.inventory.project, &path, lockfile)?;
					files.insert(path, bytes);
				}
				Fate::Delete => deleted.push(path),
			}
		}
		for (owner, loaded) in self.remotes {
			let path = remote_lockfile_path(&repo.inventory.project, &owner)?;
			let lockfile = &loaded.current;
			let empty = lockfile.documents.is_empty() && lockfile.retired.is_empty();
			match loaded.fate(empty) {
				Fate::Keep => {}
				Fate::Write => {
					let bytes = write_remote(&repo.inventory.project, &path, lockfile)?;
					files.insert(path, bytes);
				}
				Fate::Delete => deleted.push(path),
			}
		}
		for (document, declaration) in self.documents {
			let original = repo.snapshot.read(&document)?;
			let previous = self.former_documents.get(&document).unwrap_or(&document);
			let bytes = write_frontmatter(
				&repo.inventory.project,
				previous,
				&document,
				&original,
				declaration.as_ref(),
			)?;
			if bytes != original {
				files.insert(document, bytes);
			}
		}

		let existing: Vec<RepoPath> = files
			.keys()
			.filter(|path| repo.tracked.contains(*path))
			.cloned()
			.collect();
		let oids = repo.oids(&existing)?;
		let mut expected: BTreeMap<RepoPath, ObjectId> =
			existing.into_iter().zip(oids).collect();
		deleted.sort();
		let deleted_oids = repo.oids(&deleted)?;
		Ok(WritePlan {
			writes: files
				.into_iter()
				.map(|(path, contents)| PlannedWrite {
					expected: expected.remove(&path),
					path,
					contents,
				})
				.collect(),
			deletes: deleted
				.into_iter()
				.zip(deleted_oids)
				.map(|(path, expected)| PlannedDelete { path, expected })
				.collect(),
		})
	}
}

/// The canonical bytes of a local lockfile, parsed back.
fn write_local(
	project: &RepoPath,
	path: &RepoPath,
	lockfile: &LocalLockfile,
) -> Result<Vec<u8>, MutateError> {
	let invalid = |error| MutateError::InvalidLockfile {
		path: path.clone(),
		error,
	};
	let bytes = lockfile.to_bytes_in_project(project).map_err(invalid)?;
	let reread =
		LocalLockfile::parse_in_project(project, &lockfile.directory, &bytes)
			.map_err(invalid)?;
	let same = reread.documents.keys().eq(lockfile.documents.keys())
		&& reread.retired.keys().eq(lockfile.retired.keys());
	if same {
		Ok(bytes)
	} else {
		Err(MutateError::RoundTrip { path: path.clone() })
	}
}

/// The canonical bytes of a remote owner lockfile, parsed back.
fn write_remote(
	project: &RepoPath,
	path: &RepoPath,
	lockfile: &RemoteLockfile,
) -> Result<Vec<u8>, MutateError> {
	let invalid = |error| MutateError::InvalidLockfile {
		path: path.clone(),
		error,
	};
	let bytes = lockfile.to_bytes_in_project(project).map_err(invalid)?;
	let reread =
		RemoteLockfile::parse_in_project(project, &bytes).map_err(invalid)?;
	let same = reread.documents.keys().eq(lockfile.documents.keys())
		&& reread.retired.keys().eq(lockfile.retired.keys());
	if same {
		Ok(bytes)
	} else {
		Err(MutateError::RoundTrip { path: path.clone() })
	}
}

/// `original` with its declaration replaced by `declaration`, which must
/// read back unchanged.
fn write_frontmatter(
	project: &RepoPath,
	previous: &RepoPath,
	document: &RepoPath,
	original: &[u8],
	declaration: Option<&Declaration>,
) -> Result<Vec<u8>, MutateError> {
	let invalid = |error| MutateError::InvalidFrontmatter {
		document: document.clone(),
		error,
	};
	let bytes = match declaration {
		Some(declaration) if previous != document => {
			frontmatter::write_in_projects(
				project,
				project,
				previous,
				document,
				original,
				Some(declaration),
			)
		}
		_ => frontmatter::write_in_projects(
			project,
			project,
			document,
			document,
			original,
			declaration,
		),
	}
	.map_err(invalid)?;
	let reread =
		frontmatter::read_in_project(project, document, &bytes).map_err(invalid)?;
	if reread.as_ref() == declaration {
		Ok(bytes)
	} else {
		Err(MutateError::RoundTrip {
			path: document.clone(),
		})
	}
}
