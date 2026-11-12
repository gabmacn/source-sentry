//! The repository's history, read with `git2`.

use std::{collections::HashSet, fs, io};

use chrono::{DateTime, Utc};
use source_sentry_core::ports::{
	BackendError, BackendErrorKind, Commit, CommitId, FirstParent, History,
	ObjectId, RepoPath,
};

use crate::{
	error::{self, failure, is_missing_object},
	ids::object_id,
	repository::GitRepository,
};

impl History for GitRepository {
	/// Walks parent 0 from `start`, one commit per step.
	///
	/// A commit listed in the repository's shallow file (`shallow` in the
	/// common git directory) reports [`FirstParent::Shallow`]. That list is
	/// the authority, because libgit2 grafts shallow commits as parentless: a
	/// shallow commit's `parent_count()` is `0`, exactly like a root's. A
	/// commit that has no parents and is not listed reports
	/// [`FirstParent::Root`]. A parent that is missing from the object
	/// database is an error at the next step, never a root.
	///
	/// The iterator ends after the first error.
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_ {
		FirstParentWalk {
			repository: self,
			next: Some(start.clone()),
			shallow: None,
		}
	}

	/// Looks every path up in one commit's tree. A path that names a
	/// directory or a submodule (gitlink) is `None`, because neither is a blob;
	/// snapshots report submodules as `Canonical::Submodule` instead. A
	/// symbolic link is a blob holding its target.
	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		let raw = self.raw();
		let oid = self.git_oid(&commit.0).ok_or_else(|| {
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
		paths
			.iter()
			.map(|path| {
				if path.is_root() {
					return Ok(None);
				}
				match tree.get_path(std::path::Path::new(path.as_str())) {
					Ok(entry) if entry.kind() == Some(git2::ObjectType::Blob) => {
						object_id(entry.id(), self.algorithm()).map(Some)
					}
					Ok(_) => Ok(None),
					Err(source)
						if source.code() == git2::ErrorCode::NotFound
							&& !is_missing_object(&source) =>
					{
						Ok(None)
					}
					Err(source) => Err(error::git(
						format!("cannot look up {path} in {commit}"),
						source,
					)),
				}
			})
			.collect()
	}

	/// Looks `directory` up in each commit's tree, and lists a subtree only
	/// the first time it is seen, so that a directory that rarely changes
	/// costs one lookup per commit.
	fn files_ever_in(
		&self,
		start: &CommitId,
		directory: &RepoPath,
	) -> Result<Vec<RepoPath>, BackendError> {
		let raw = self.raw();
		let mut seen = HashSet::new();
		let mut files = std::collections::BTreeSet::new();
		for commit in self.first_parent_walk(start) {
			let commit = commit?;
			let oid = self.git_oid(&commit.id.0).ok_or_else(|| {
				failure(
					BackendErrorKind::MissingObject,
					format!("the commit {} is missing", commit.id),
				)
			})?;
			let tree = raw
				.find_commit(oid)
				.and_then(|found| found.tree())
				.map_err(|source| {
					error::git(format!("cannot read the tree of {}", commit.id), source)
				})?;
			let Ok(entry) = tree.get_path(std::path::Path::new(directory.as_str()))
			else {
				continue;
			};
			if entry.kind() != Some(git2::ObjectType::Tree)
				|| !seen.insert(entry.id())
			{
				continue;
			}
			let subtree = raw.find_tree(entry.id()).map_err(|source| {
				error::git(format!("cannot read {directory} in {}", commit.id), source)
			})?;
			for item in &subtree {
				if item.kind() == Some(git2::ObjectType::Blob)
					&& let Ok(name) = item.name()
					&& let Ok(path) = directory.join(name)
				{
					files.insert(path);
				}
			}
		}
		Ok(files.into_iter().collect())
	}

	/// Reads a blob. `None` means the object database has no blob with this
	/// ID: the object is missing, is another kind of object, or uses another
	/// hash algorithm.
	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError> {
		self.blob_content(id)
	}
}

/// The lazy iterator behind [`History::first_parent_walk`].
struct FirstParentWalk<'repo> {
	repository: &'repo GitRepository,
	next: Option<CommitId>,
	/// The shallow boundary, read at the first step.
	shallow: Option<HashSet<String>>,
}

impl Iterator for FirstParentWalk<'_> {
	type Item = Result<Commit, BackendError>;

	fn next(&mut self) -> Option<Self::Item> {
		let id = self.next.take()?;
		let result = self.step(id);
		if let Ok(commit) = &result
			&& let FirstParent::Commit(parent) = &commit.first_parent
		{
			self.next = Some(parent.clone());
		}
		Some(result)
	}
}

impl FirstParentWalk<'_> {
	/// Reads one commit and decides where the chain continues.
	fn step(&mut self, id: CommitId) -> Result<Commit, BackendError> {
		let algorithm = self.repository.algorithm();
		let oid = self.repository.git_oid(&id.0).ok_or_else(|| {
			failure(
				BackendErrorKind::MissingObject,
				format!("the commit {id} is missing"),
			)
		})?;
		let commit = self.repository.raw().find_commit(oid).map_err(|source| {
			error::git(format!("cannot read the commit {id}"), source)
		})?;
		let seconds = commit.committer().when().seconds();
		let committer_time = DateTime::<Utc>::from_timestamp(seconds, 0)
			.ok_or_else(|| {
				failure(
					BackendErrorKind::Git,
					format!(
						"the commit {id} has an out-of-range committer date ({seconds})"
					),
				)
			})?;
		let first_parent = if self.shallow_commits()?.contains(id.0.hex()) {
			FirstParent::Shallow
		} else if commit.parent_count() == 0 {
			FirstParent::Root
		} else {
			let parent = commit.parent_id(0).map_err(|source| {
				error::git(format!("cannot read the parents of {id}"), source)
			})?;
			FirstParent::Commit(CommitId(object_id(parent, algorithm)?))
		};
		Ok(Commit {
			id,
			committer_time,
			first_parent,
		})
	}

	/// The commits whose parents a shallow clone cut off.
	fn shallow_commits(&mut self) -> Result<&HashSet<String>, BackendError> {
		if self.shallow.is_none() {
			self.shallow = Some(read_shallow(self.repository)?);
		}
		Ok(self.shallow.get_or_insert_default())
	}
}

/// Reads the shallow file: one commit ID per line, absent in a complete
/// repository.
fn read_shallow(
	repository: &GitRepository,
) -> Result<HashSet<String>, BackendError> {
	let path = repository.common_dir().join("shallow");
	match fs::read_to_string(&path) {
		Ok(text) => Ok(
			text
				.lines()
				.map(str::trim)
				.filter(|line| !line.is_empty())
				.map(str::to_ascii_lowercase)
				.collect(),
		),
		Err(source) if source.kind() == io::ErrorKind::NotFound => {
			Ok(HashSet::new())
		}
		Err(source) => {
			Err(error::io(format!("cannot read {}", path.display()), source))
		}
	}
}
