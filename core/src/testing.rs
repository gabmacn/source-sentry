//! In-memory fakes of [`Snapshot`] and [`History`], for tests.
//!
//! Compiled for the crate's own tests, and for other crates through the
//! `testing` feature. Identical content always gets the same fake oid, as in
//! git, so a fake snapshot and a fake history agree about which files are
//! unchanged.

use std::{collections::BTreeMap, fmt::Write as _};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::ports::{
	BackendError, BackendErrorKind, Canonical, Commit, CommitId, FirstParent,
	HashAlgorithm, History, ObjectId, RepoPath, Revision, Snapshot,
};

/// A deterministic fake oid for `content`: the first 20 bytes of its SHA-256,
/// labelled as SHA-1. It is not git's real blob oid, but equal content always
/// yields an equal oid.
///
/// # Panics
///
/// Never in practice: 20 bytes always format as the 40 lowercase hexadecimal
/// digits that a SHA-1 ID requires.
#[must_use]
pub fn fake_oid(content: &[u8]) -> ObjectId {
	let digest = Sha256::digest(content);
	let hex = digest[..20].iter().fold(String::new(), |mut hex, byte| {
		// Writing to a `String` cannot fail.
		let _ = write!(hex, "{byte:02x}");
		hex
	});
	#[expect(
		clippy::expect_used,
		reason = "20 bytes always format as 40 lowercase hexadecimal digits"
	)]
	ObjectId::new(HashAlgorithm::Sha1, &hex).expect("valid fake oid")
}

/// Parses a path in test setup.
///
/// # Panics
///
/// Panics when `path` is not a valid repository path, which is a mistake in
/// the test itself.
#[must_use]
pub fn path(path: &str) -> RepoPath {
	#[expect(
		clippy::expect_used,
		reason = "test helper: an invalid path is a bug in the test"
	)]
	RepoPath::new(path).expect("valid test path")
}

/// One commit of a [`FakeHistory`].
#[derive(Clone, Debug)]
struct FakeCommit {
	id: CommitId,
	time: DateTime<Utc>,
	tree: BTreeMap<RepoPath, ObjectId>,
}

/// A linear first-parent history held in memory.
///
/// Commits are added oldest first with [`FakeHistory::commit`]. Merged
/// branches need no representation, because Source Sentry only walks the
/// first-parent chain.
#[derive(Clone, Debug, Default)]
pub struct FakeHistory {
	commits: Vec<FakeCommit>,
	blobs: BTreeMap<ObjectId, Vec<u8>>,
	shallow_at: Option<usize>,
}

impl FakeHistory {
	/// An empty history: a repository without commits.
	#[must_use]
	pub fn new() -> Self {
		Self::default()
	}

	/// Adds a commit on top of the current head and returns its ID.
	///
	/// Each change writes a file (`Some(content)`) or deletes it (`None`);
	/// every other file carries over from the previous commit.
	pub fn commit(
		&mut self,
		time: DateTime<Utc>,
		changes: &[(&str, Option<&[u8]>)],
	) -> CommitId {
		let mut tree = self
			.commits
			.last()
			.map(|commit| commit.tree.clone())
			.unwrap_or_default();
		for (file, content) in changes {
			match content {
				Some(content) => {
					let oid = fake_oid(content);
					self.blobs.insert(oid.clone(), content.to_vec());
					tree.insert(path(file), oid);
				}
				None => {
					tree.remove(&path(file));
				}
			}
		}
		let index = self.commits.len();
		let id = CommitId(fake_oid(format!("commit {index}").as_bytes()));
		self.commits.push(FakeCommit {
			id: id.clone(),
			time,
			tree,
		});
		id
	}

	/// The newest commit, or `None` when there are no commits.
	#[must_use]
	pub fn head(&self) -> Option<CommitId> {
		self.commits.last().map(|commit| commit.id.clone())
	}

	/// Makes `id` the oldest available commit, as a shallow clone would: its
	/// parent, and everything before it, can no longer be walked.
	///
	/// # Panics
	///
	/// Panics when `id` is not a commit of this history.
	pub fn make_shallow_at(&mut self, id: &CommitId) {
		#[expect(
			clippy::expect_used,
			reason = "test helper: an unknown commit is a bug in the test"
		)]
		let index = self.index_of(id).expect("known commit");
		self.shallow_at = Some(index);
	}

	/// Removes a blob's content, as if the object database lacked it.
	pub fn forget_blob(&mut self, id: &ObjectId) {
		self.blobs.remove(id);
	}

	/// The files of the commit `id`, by path.
	///
	/// # Panics
	///
	/// Panics when `id` is not a commit of this history.
	#[must_use]
	pub fn tree(&self, id: &CommitId) -> &BTreeMap<RepoPath, ObjectId> {
		#[expect(
			clippy::expect_used,
			reason = "test helper: an unknown commit is a bug in the test"
		)]
		let index = self.index_of(id).expect("known commit");
		&self.commits[index].tree
	}

	fn index_of(&self, id: &CommitId) -> Option<usize> {
		self.commits.iter().position(|commit| &commit.id == id)
	}

	fn missing(id: &CommitId) -> BackendError {
		BackendError::new(
			BackendErrorKind::MissingObject,
			format!("commit {id} is not in the fake history"),
		)
	}
}

impl History for FakeHistory {
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_ {
		let start_index = self.index_of(start);
		let missing = start_index.is_none().then(|| Err(Self::missing(start)));
		let floor = self.shallow_at.unwrap_or(0);
		let indices = start_index
			.map(|index| (floor.min(index)..=index).rev())
			.into_iter()
			.flatten();
		let commits = indices.map(move |index| {
			let commit = &self.commits[index];
			let first_parent = if Some(index) == self.shallow_at {
				FirstParent::Shallow
			} else if index == 0 {
				FirstParent::Root
			} else {
				FirstParent::Commit(self.commits[index - 1].id.clone())
			};
			Ok(Commit {
				id: commit.id.clone(),
				committer_time: commit.time,
				first_parent,
			})
		});
		missing.into_iter().chain(commits)
	}

	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		let index = self.index_of(commit).ok_or_else(|| Self::missing(commit))?;
		if self.shallow_at.is_some_and(|floor| index < floor) {
			return Err(Self::missing(commit));
		}
		let tree = &self.commits[index].tree;
		Ok(paths.iter().map(|path| tree.get(path).cloned()).collect())
	}

	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError> {
		Ok(self.blobs.get(id).cloned())
	}
}

/// The files of one snapshot, held in memory.
#[derive(Clone, Debug)]
pub struct FakeSnapshot {
	revision: Revision,
	files: BTreeMap<RepoPath, Vec<u8>>,
	canonical: BTreeMap<RepoPath, Canonical>,
}

impl FakeSnapshot {
	/// An empty working tree on top of `head`.
	#[must_use]
	pub fn working_tree(head: Option<CommitId>) -> Self {
		Self {
			revision: Revision::WorkingTree { head },
			files: BTreeMap::new(),
			canonical: BTreeMap::new(),
		}
	}

	/// The working tree of a clean checkout of `history`'s head.
	#[must_use]
	pub fn checkout(history: &FakeHistory) -> Self {
		let mut snapshot = Self::working_tree(history.head());
		if let Some(head) = history.head() {
			for (file, oid) in history.tree(&head) {
				let content = history.blobs.get(oid).cloned().unwrap_or_default();
				snapshot.files.insert(file.clone(), content);
			}
		}
		snapshot
	}

	/// Adds or replaces a file.
	#[must_use]
	pub fn with_file(mut self, file: &str, content: &[u8]) -> Self {
		self.files.insert(path(file), content.to_vec());
		self
	}

	/// Removes a file.
	#[must_use]
	pub fn without_file(mut self, file: &str) -> Self {
		self.files.remove(&path(file));
		self
	}

	/// Overrides what [`Snapshot::canonical_oids`] reports for `file`, for
	/// example an unsupported filter.
	#[must_use]
	pub fn with_canonical(mut self, file: &str, canonical: Canonical) -> Self {
		self.canonical.insert(path(file), canonical);
		self
	}
}

impl Snapshot for FakeSnapshot {
	fn revision(&self) -> &Revision {
		&self.revision
	}

	fn paths(&self) -> Result<Vec<RepoPath>, BackendError> {
		Ok(self.files.keys().cloned().collect())
	}

	fn read(&self, file: &RepoPath) -> Result<Vec<u8>, BackendError> {
		self.files.get(file).cloned().ok_or_else(|| {
			BackendError::new(
				BackendErrorKind::Io,
				format!("{file} is not in the fake snapshot"),
			)
		})
	}

	fn canonical_oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, BackendError> {
		paths
			.iter()
			.map(|file| {
				if let Some(canonical) = self.canonical.get(file) {
					return Ok(canonical.clone());
				}
				self
					.read(file)
					.map(|content| Canonical::Oid(fake_oid(&content)))
			})
			.collect()
	}
}
