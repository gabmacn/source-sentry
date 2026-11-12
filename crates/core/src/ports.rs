//! The traits through which the core reads a repository.
//!
//! The traits expose git primitives only. Every rule (the newest matching
//! blob, the suffix minimum of committer dates, the 24-hour tolerance, exit 4
//! at a shallow boundary, cadence) lives in the core, where in-memory fakes of
//! these traits test it. The git backend (`source-sentry-git`) implements them
//! with `git2`, `git hash-object` and native LFS pointers.
//!
//! The core takes them as generic parameters, for example
//! `fn check<S: Snapshot, H: History>(snapshot: &S, history: &H, at: DateTime<Utc>, context: &str)`.
//! `History` returns an `impl Iterator`, so the traits are not object-safe; generics are intended.

use chrono::{DateTime, Utc};

// The value types that cross the boundary are defined in `model`, which owns
// their validation, and re-exported here so the traits read on their own.
pub use crate::model::{CommitId, HashAlgorithm, ObjectId, RepoPath};

/// What a snapshot shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revision {
	/// The working tree on top of `head`, which is `None` in a repository without commits.
	WorkingTree {
		/// The commit the working tree is based on.
		head: Option<CommitId>,
	},
	/// A committed snapshot, selected with `--rev`.
	Commit(CommitId),
}

/// One path's canonical blob oid, or the reason it could not be computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Canonical {
	/// The blob git would store for this path.
	Oid(ObjectId),
	/// An external clean or process filter applies, so no oid was guessed.
	UnsupportedFilter {
		/// The filter driver's name.
		filter: String,
		/// Where the attribute was set, for example `.gitattributes:3`.
		attribute_origin: String,
	},
	/// A Git LFS pointer uses extensions, which Source Sentry rejects.
	UnsupportedLfsExtension,
	/// The index holds the file with CRLF line endings although its attributes
	/// ask git to normalize it, so `git hash-object` and `git add` would
	/// disagree. Running `git add --renormalize .` resolves it.
	CrlfInIndex,
	/// The path is a submodule, which Source Sentry does not support.
	Submodule,
}

/// The files of one snapshot: the working tree, or a commit's tree.
pub trait Snapshot {
	/// The revision this snapshot shows.
	fn revision(&self) -> &Revision;

	/// Every tracked path, sorted. For the working tree this is the index's
	/// membership, where a path omitted by sparse checkout is still present.
	///
	/// # Errors
	///
	/// Fails when the backend cannot read the index or the tree.
	fn paths(&self) -> Result<Vec<RepoPath>, BackendError>;

	/// A file's bytes as this snapshot shows them: working-tree bytes, or blob content.
	///
	/// # Errors
	///
	/// Fails when the file cannot be read.
	fn read(&self, path: &RepoPath) -> Result<Vec<u8>, BackendError>;

	/// A file's content as the blob `canonical` holds it, for showing
	/// changes, or `None` when it cannot be had. A snapshot whose content is
	/// already canonical returns [`Snapshot::read`]. The working tree undoes
	/// git's line-ending conversion where that reproduces `canonical`, or
	/// else reads the blob from the object database, which holds it once the
	/// file is staged; a conversion such as `working-tree-encoding` gives
	/// `None` until then.
	///
	/// # Errors
	///
	/// Fails when the file or the object database cannot be read.
	fn read_canonical(
		&self,
		path: &RepoPath,
		canonical: &ObjectId,
	) -> Result<Option<Vec<u8>>, BackendError> {
		let _ = canonical;
		self.read(path).map(Some)
	}

	/// Canonical blob oids for many paths at once, in the order given.
	///
	/// # Errors
	///
	/// Fails only when the backend itself fails. A path that cannot be canonicalized is reported in
	/// its own [`Canonical`] value instead.
	fn canonical_oids(
		&self,
		paths: &[RepoPath],
	) -> Result<Vec<Canonical>, BackendError>;
}

/// Where a commit's first-parent chain continues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirstParent {
	/// The chain continues with this commit.
	Commit(CommitId),
	/// The commit has no parents, so the walk has seen the complete history.
	Root,
	/// A shallow clone cut the parent off. Exit 4 if the walk needed to go
	/// further.
	Shallow,
}

/// One commit on a first-parent walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
	/// The commit's ID.
	pub id: CommitId,
	/// The committer date, which the suffix minimum over later commits uses.
	pub committer_time: DateTime<Utc>,
	/// Where the walk continues.
	pub first_parent: FirstParent,
}

/// The repository's history.
pub trait History {
	/// Walks the first-parent chain from `start`, newest first, lazily.
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_;

	/// The blob at each path in a commit's tree, in the order given, or `None` where the path is
	/// absent.
	///
	/// # Errors
	///
	/// Fails when the commit or its tree cannot be read.
	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError>;

	/// A blob's content, or `None` when the object database lacks it. `diff` then
	/// reports "baseline unavailable".
	///
	/// # Errors
	///
	/// Fails when the object database cannot be read.
	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError>;

	/// Every file directly inside `directory` in any commit of the
	/// first-parent chain from `start`, sorted, including files deleted
	/// since. The walk ends quietly at a shallow boundary.
	///
	/// # Errors
	///
	/// Fails when a commit or tree cannot be read.
	fn files_ever_in(
		&self,
		start: &CommitId,
		directory: &RepoPath,
	) -> Result<Vec<RepoPath>, BackendError>;
}

/// A failure of the backend itself. It always means the evaluation could not
/// complete: exit 4, which no context can downgrade.
#[derive(Debug, thiserror::Error)]
#[error("the repository backend failed ({kind})")]
pub struct BackendError {
	/// What failed.
	pub kind: BackendErrorKind,
	/// The underlying error.
	#[source]
	pub source: Box<dyn std::error::Error + Send + Sync>,
}

impl BackendError {
	/// Wraps `source` as a backend failure of the given kind.
	pub fn new(
		kind: BackendErrorKind,
		source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
	) -> Self {
		Self {
			kind,
			source: source.into(),
		}
	}
}

/// The kinds of backend failure, which decide the recipe the report suggests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackendErrorKind {
	/// Reading a file failed.
	#[error("a file could not be read")]
	Io,
	/// The repository could not be read.
	#[error("the repository could not be read")]
	Git,
	/// A required object is missing from the object database.
	#[error("a required object is missing")]
	MissingObject,
	/// The `git` executable is missing or failed.
	#[error("the git executable is missing or failed")]
	GitExecutable,
}

/// Files that a mutation command wants written. The core produces the plan and
/// never writes; the backend applies it atomically.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WritePlan {
	/// The writes, which are applied all together or not at all.
	pub writes: Vec<PlannedWrite>,
	/// The files to delete, applied together with the writes, all or nothing. A
	/// path may not appear both here and in `writes`.
	pub deletes: Vec<PlannedDelete>,
}

/// One file that a [`WritePlan`] deletes, such as a lockfile that a move
/// emptied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedDelete {
	/// The file to delete.
	pub path: RepoPath,
	/// The canonical oid the file must still have when the plan is applied. A
	/// mismatch aborts the whole plan.
	pub expected: ObjectId,
}

/// One file in a [`WritePlan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedWrite {
	/// The file to write: a lockfile, or a document whose frontmatter changes.
	pub path: RepoPath,
	/// The canonical oid the file must still have when the plan is applied, or `None` when the file
	/// must not exist yet. A mismatch aborts the whole plan.
	pub expected: Option<ObjectId>,
	/// The file's new contents.
	pub contents: Vec<u8>,
}
