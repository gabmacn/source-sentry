//! The two snapshots: the working tree, and a committed tree.

use std::{collections::BTreeMap, fs};

use source_sentry_core::ports::{
	BackendError, BackendErrorKind, Canonical, CommitId, ObjectId, RepoPath,
	Revision, Snapshot,
};

use crate::{
	canonical::Canonicalizer,
	error::{self, failure},
	ids::{blob_oid, object_id},
	index::{IndexRecord, MODE_GITLINK, MODE_SYMLINK, is_regular, repo_path},
	presence::{Presence, Probe, native},
	repository::GitRepository,
};

/// The working tree on top of `HEAD`, as `check` evaluates it by default.
///
/// - Membership is the index's, captured when the snapshot was made: only
///   tracked paths count. A path that sparse checkout marks skip-worktree is
///   present, and its index blob stands for it. A tracked path that the
///   working tree no longer holds is absent, as an unstaged deletion. A path
///   marked intent-to-add, as [`GitRepository::apply`] marks the files it
///   creates, is a member. Its index entry holds the empty blob,
///   so, like every working-tree file, it is hashed from the working tree.
/// - A submodule (gitlink) is a member while its directory exists, so that
///   the core can report it. Its canonical value is
///   [`Canonical::Submodule`]; it has no content, so [`Snapshot::read`]
///   fails for it.
/// - Content and canonical oids are read from the working tree when asked
///   for. A symbolic link is never followed: its content is its target.
#[derive(Debug)]
pub struct WorkingTreeSnapshot<'repo> {
	repository: &'repo GitRepository,
	revision: Revision,
	index: BTreeMap<RepoPath, IndexRecord>,
}

impl<'repo> WorkingTreeSnapshot<'repo> {
	/// A snapshot over `index`, based on `head`.
	pub(crate) fn new(
		repository: &'repo GitRepository,
		head: Option<CommitId>,
		index: BTreeMap<RepoPath, IndexRecord>,
	) -> Self {
		Self {
			repository,
			revision: Revision::WorkingTree { head },
			index,
		}
	}

	/// The index record of a member path, submodules included.
	fn member(&self, path: &RepoPath) -> Result<&IndexRecord, BackendError> {
		self.index.get(path).ok_or_else(|| not_a_member(path))
	}
}

impl Snapshot for WorkingTreeSnapshot<'_> {
	fn revision(&self) -> &Revision {
		&self.revision
	}

	fn paths(&self) -> Result<Vec<RepoPath>, BackendError> {
		let work_dir = self.repository.require_work_dir()?;
		let mut probe = Probe::new(work_dir);
		let mut paths = Vec::with_capacity(self.index.len());
		for record in self.index.values() {
			let present = if record.skip_worktree {
				true
			} else if record.is_gitlink() {
				probe.holds_directory(&record.path)?
			} else {
				probe.presence(&record.path)? != Presence::Absent
			};
			if present {
				paths.push(record.path.clone());
			}
		}
		Ok(paths)
	}

	fn read(&self, path: &RepoPath) -> Result<Vec<u8>, BackendError> {
		let record = self.member(path)?;
		if record.is_gitlink() {
			return Err(submodule(path));
		}
		if record.skip_worktree {
			return self.repository.required_blob(&record.oid, path);
		}
		let work_dir = self.repository.require_work_dir()?;
		match Probe::new(work_dir).presence(path)? {
			Presence::File => fs::read(native(work_dir, path))
				.map_err(|source| error::io(format!("cannot read {path}"), source)),
			Presence::Symlink(target) => Ok(target),
			Presence::Absent => Err(absent(path)),
		}
	}

	/// Under `core.autocrlf` or a `text` attribute, the working tree holds
	/// CRLF where the blob holds LF, and showing those bytes would mark every
	/// line as changed. The CRLF-to-LF conversion is undone only when the
	/// result hashes to `canonical`, so other conversions (`ident`,
	/// `working-tree-encoding`, LFS) never produce a wrong text.
	fn read_canonical(
		&self,
		path: &RepoPath,
		canonical: &ObjectId,
	) -> Result<Option<Vec<u8>>, BackendError> {
		let bytes = self.read(path)?;
		let algorithm = self.repository.algorithm();
		if blob_oid(algorithm, &bytes)? == *canonical {
			return Ok(Some(bytes));
		}
		if bytes.windows(2).any(|pair| pair == b"\r\n") {
			let converted = crlf_to_lf(&bytes);
			if blob_oid(algorithm, &converted)? == *canonical {
				return Ok(Some(converted));
			}
		}
		// Another conversion, such as `working-tree-encoding`: the object
		// database holds the canonical blob once the file is staged.
		self.repository.blob_content(canonical)
	}

	fn canonical_oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, BackendError> {
		let work_dir = self.repository.require_work_dir()?;
		let mut probe = Probe::new(work_dir);
		let mut known = Vec::with_capacity(paths.len());
		let mut regular = Vec::new();
		for path in paths {
			let record = self.member(path)?;
			let canonical = if record.is_gitlink() {
				Some(Canonical::Submodule)
			} else if record.skip_worktree {
				Some(Canonical::Oid(record.oid.clone()))
			} else {
				match probe.presence(path)? {
					Presence::Symlink(target) => Some(Canonical::Oid(blob_oid(
						self.repository.algorithm(),
						&target,
					)?)),
					// A placeholder under `core.symlinks=false`: git stores its
					// bytes as the target, without attributes or filters.
					Presence::File if record.is_symlink() => {
						let target =
							fs::read(native(work_dir, path)).map_err(|source| {
								error::io(format!("cannot read {path}"), source)
							})?;
						Some(Canonical::Oid(blob_oid(
							self.repository.algorithm(),
							&target,
						)?))
					}
					Presence::File => {
						regular.push(path);
						None
					}
					Presence::Absent => return Err(absent(path)),
				}
			};
			known.push(canonical);
		}
		let mut hashed = Canonicalizer::new(self.repository, &self.index)
			.regular_files(&regular)?
			.into_iter();
		known
			.into_iter()
			.map(|canonical| {
				canonical.or_else(|| hashed.next()).ok_or_else(|| {
					failure(
						BackendErrorKind::GitExecutable,
						"fewer canonical oids than files",
					)
				})
			})
			.collect()
	}
}

/// A commit's tree, as `check --rev` evaluates it.
///
/// Its members are the tree's blobs, symbolic links included, and its
/// submodules (gitlinks), so that the core can report them.
/// Canonical oids are the tree's blob oids, which need no hashing; a
/// submodule's canonical value is [`Canonical::Submodule`], and
/// [`Snapshot::read`] fails for it.
#[derive(Debug)]
pub struct CommitSnapshot<'repo> {
	repository: &'repo GitRepository,
	revision: Revision,
	files: BTreeMap<RepoPath, TreeFile>,
}

/// One member of a committed tree.
#[derive(Clone, Debug)]
enum TreeFile {
	/// A file or symbolic link, with its blob.
	Blob(ObjectId),
	/// A submodule, which has no blob.
	Submodule,
}

impl<'repo> CommitSnapshot<'repo> {
	/// Reads the complete tree of `commit`.
	pub(crate) fn new(
		repository: &'repo GitRepository,
		commit: CommitId,
	) -> Result<Self, BackendError> {
		let algorithm = repository.algorithm();
		let raw = repository.raw();
		let oid = repository.git_oid(&commit.0).ok_or_else(|| {
			failure(
				BackendErrorKind::MissingObject,
				format!("the commit {commit} is missing"),
			)
		})?;
		let tree = raw
			.find_commit(oid)
			.and_then(|found| found.tree())
			.map_err(|source| {
				error::git(format!("cannot read the tree of {commit}"), source)
			})?;
		let mut files = BTreeMap::new();
		let mut pending = vec![(RepoPath::ROOT, tree)];
		while let Some((directory, tree)) = pending.pop() {
			for entry in &tree {
				let name = repo_path(entry.name_bytes())?;
				let path = directory.join(name.as_str()).map_err(|source| {
					error::caused(
						BackendErrorKind::Git,
						format!("the tree of {commit} holds an invalid path"),
						source,
					)
				})?;
				let mode = u32::try_from(entry.filemode()).unwrap_or_default();
				if is_regular(mode) || mode == MODE_SYMLINK {
					files.insert(path, TreeFile::Blob(object_id(entry.id(), algorithm)?));
				} else if mode == MODE_GITLINK {
					files.insert(path, TreeFile::Submodule);
				} else {
					let subtree = raw.find_tree(entry.id()).map_err(|source| {
						error::git(
							format!("cannot read the directory {path} of {commit}"),
							source,
						)
					})?;
					pending.push((path, subtree));
				}
			}
		}
		Ok(Self {
			repository,
			revision: Revision::Commit(commit),
			files,
		})
	}

	/// A member path's entry.
	fn member(&self, path: &RepoPath) -> Result<&TreeFile, BackendError> {
		self.files.get(path).ok_or_else(|| not_a_member(path))
	}
}

impl Snapshot for CommitSnapshot<'_> {
	fn revision(&self) -> &Revision {
		&self.revision
	}

	fn paths(&self) -> Result<Vec<RepoPath>, BackendError> {
		Ok(self.files.keys().cloned().collect())
	}

	fn read(&self, path: &RepoPath) -> Result<Vec<u8>, BackendError> {
		match self.member(path)? {
			TreeFile::Blob(id) => self.repository.required_blob(id, path),
			TreeFile::Submodule => Err(submodule(path)),
		}
	}

	fn canonical_oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, BackendError> {
		paths
			.iter()
			.map(|path| {
				Ok(match self.member(path)? {
					TreeFile::Blob(id) => Canonical::Oid(id.clone()),
					TreeFile::Submodule => Canonical::Submodule,
				})
			})
			.collect()
	}
}

/// The error for a path the snapshot does not contain.
fn not_a_member(path: &RepoPath) -> BackendError {
	failure(
		BackendErrorKind::Io,
		format!("{path} is not a file of this snapshot"),
	)
}

/// The error for reading a submodule, which has no content of its own.
fn submodule(path: &RepoPath) -> BackendError {
	failure(
		BackendErrorKind::Io,
		format!("{path} is a submodule, which has no file content"),
	)
}

/// The error for a member path that the working tree no longer holds.
fn absent(path: &RepoPath) -> BackendError {
	failure(
		BackendErrorKind::Io,
		format!("{path} is no longer in the working tree"),
	)
}

/// `bytes` with every CRLF pair replaced by LF, as git's `text` conversion
/// stores a file. A lone CR is kept.
fn crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
	let mut converted = Vec::with_capacity(bytes.len());
	let mut rest = bytes.iter().peekable();
	while let Some(&byte) = rest.next() {
		if byte == b'\r' && rest.peek() == Some(&&b'\n') {
			continue;
		}
		converted.push(byte);
	}
	converted
}

#[cfg(test)]
mod tests {
	use super::crlf_to_lf;

	#[test]
	fn crlf_to_lf_keeps_lone_carriage_returns() {
		assert_eq!(crlf_to_lf(b"a\r\nb\rc\r\n\r\n"), b"a\nb\rc\n\n");
		assert_eq!(crlf_to_lf(b"\r"), b"\r");
		assert_eq!(crlf_to_lf(b""), b"");
	}
}
