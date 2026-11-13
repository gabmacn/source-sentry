//! An open repository: the entry point of the backend.

use std::path::{Path, PathBuf};

use source_sentry_core::ports::{
	BackendError, BackendErrorKind, CommitId, HashAlgorithm, ObjectId, RepoPath,
};

use crate::{
	command::{Git, exit_failure},
	error::{self, caused, failure},
	ids::{algorithm_of_format, git_oid, object_id},
	index::read_index,
	snapshot::{CommitSnapshot, WorkingTreeSnapshot},
};

/// A git repository opened for Source Sentry.
///
/// It implements [`History`](source_sentry_core::ports::History), produces
/// [`Snapshot`](source_sentry_core::ports::Snapshot)s, and applies write plans
/// with [`GitRepository::apply`].
///
/// # Requirements
///
/// The `git` executable must be on the `PATH` for working-tree snapshots,
/// write plans and [`GitRepository::default_reviewer`]. History and
/// committed snapshots need only `git2`.
///
/// Both SHA-1 and SHA-256 repositories can be opened. SHA-256
/// needs `git2`'s `unstable-sha256` feature, which the workspace enables;
/// object IDs carry the repository's algorithm throughout.
pub struct GitRepository {
	repository: git2::Repository,
	git: Git,
	format: git2::ObjectFormat,
	algorithm: HashAlgorithm,
	work_dir: Option<PathBuf>,
	common_dir: PathBuf,
}

impl std::fmt::Debug for GitRepository {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter
			.debug_struct("GitRepository")
			.field("git_dir", &self.repository.path())
			.field("work_dir", &self.work_dir)
			.field("algorithm", &self.algorithm)
			.finish_non_exhaustive()
	}
}

impl GitRepository {
	/// Opens the repository that contains `path`, searching upwards from it as
	/// git does, so that a command run from a subdirectory finds its
	/// repository.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::Git`] when no repository contains
	/// `path`, or when git refuses it, for example because another user owns
	/// it.
	pub fn open(path: impl AsRef<Path>) -> Result<Self, BackendError> {
		let path = path.as_ref();
		let repository = git2::Repository::discover(path).map_err(|source| {
			let hint = if source.message().contains("object format") {
				"; reading SHA-256 repositories needs git2's `unstable-sha256` \
				 feature"
			} else {
				""
			};
			caused(
				BackendErrorKind::Git,
				format!("cannot open a git repository at {}{hint}", path.display()),
				source,
			)
		})?;
		let format = repository.object_format();
		let algorithm = algorithm_of_format(format)?;
		let work_dir = repository.workdir().map(Path::to_path_buf);
		let common_dir = repository.commondir().to_path_buf();
		let git = Git::new(repository.path(), work_dir.as_deref());
		Ok(Self {
			repository,
			git,
			format,
			algorithm,
			work_dir,
			common_dir,
		})
	}

	/// The algorithm of the repository's object IDs.
	#[must_use]
	pub fn algorithm(&self) -> HashAlgorithm {
		self.algorithm
	}

	/// The working tree, or `None` for a bare repository.
	#[must_use]
	pub fn work_dir(&self) -> Option<&Path> {
		self.work_dir.as_deref()
	}

	/// The commit `HEAD` points to, or `None` when the current branch has no
	/// commits yet.
	///
	/// # Errors
	///
	/// Fails when `HEAD` cannot be read, or points to something that is not a
	/// commit.
	pub fn head(&self) -> Result<Option<CommitId>, BackendError> {
		let head = match self.repository.head() {
			Ok(head) => head,
			Err(source) if source.code() == git2::ErrorCode::UnbornBranch => {
				return Ok(None);
			}
			Err(source) => return Err(error::git("cannot read HEAD", source)),
		};
		let commit = head.peel_to_commit().map_err(|source| {
			error::git("HEAD does not point to a commit", source)
		})?;
		Ok(Some(CommitId(object_id(commit.id(), self.algorithm)?)))
	}

	/// Resolves a revision, such as a branch, a tag or `HEAD~2`, to a commit.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::Git`] when `revision` names nothing or
	/// something that is not a commit, and with
	/// [`BackendErrorKind::MissingObject`] when the object it names is
	/// missing.
	pub fn resolve(&self, revision: &str) -> Result<CommitId, BackendError> {
		let commit = self
			.repository
			.revparse_single(revision)
			.and_then(|object| object.peel_to_commit())
			.map_err(|source| {
				error::git(format!("cannot resolve the revision `{revision}`"), source)
			})?;
		Ok(CommitId(object_id(commit.id(), self.algorithm)?))
	}

	/// A snapshot of the working tree on top of `HEAD`.
	///
	/// Its membership is the index's at this moment: tracked paths only, with
	/// paths that sparse checkout omits present through their index blobs.
	/// File contents are read when asked for.
	///
	/// # Errors
	///
	/// Fails for a bare repository, when `HEAD` cannot be read, and when
	/// `git ls-files` fails or lists a path that Source Sentry cannot
	/// represent.
	pub fn working_tree_snapshot(
		&self,
	) -> Result<WorkingTreeSnapshot<'_>, BackendError> {
		self.require_work_dir()?;
		let head = self.head()?;
		let index = read_index(&self.git, self.algorithm)?;
		Ok(WorkingTreeSnapshot::new(self, head, index))
	}

	/// A snapshot of a commit's tree, for `--rev`. `revision` is
	/// resolved like [`GitRepository::resolve`].
	///
	/// # Errors
	///
	/// Fails like [`GitRepository::resolve`], and when the commit's tree
	/// cannot be read completely.
	pub fn commit_snapshot(
		&self,
		revision: &str,
	) -> Result<CommitSnapshot<'_>, BackendError> {
		let commit = self.resolve(revision)?;
		CommitSnapshot::new(self, commit)
	}

	/// The name recorded by default for a review, a seal, a retirement or a
	/// rename: `source-sentry.reviewer` from git's configuration when it is
	/// set, or else git's `user.name`; `None` when both are unset or empty.
	/// It reads git's own configuration, so every level and include git
	/// honours applies.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::GitExecutable`] when `git` cannot run or
	/// fails, and with [`BackendErrorKind::Git`] when a name is not UTF-8.
	pub fn default_reviewer(&self) -> Result<Option<String>, BackendError> {
		let args = [
			"config",
			"--null",
			"--get-regexp",
			r"^(source-sentry\.reviewer|user\.name)$",
		];
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => {}
			// Git exits with 1 when neither key is set.
			Some(1) => return Ok(None),
			_ => return Err(exit_failure(&args, &finished)),
		}
		let listed = String::from_utf8(finished.stdout).map_err(|source| {
			caused(
				BackendErrorKind::Git,
				"git's configured name is not UTF-8",
				source,
			)
		})?;
		// Each entry is `<key>\n<value>` and ends with a NUL. A key set at
		// several levels is listed once for each; git uses the last.
		let named = |key: &str| {
			listed
				.split('\0')
				.filter_map(|entry| entry.split_once('\n'))
				.filter(|(name, value)| *name == key && !value.trim().is_empty())
				.map(|(_, value)| value.to_owned())
				.next_back()
		};
		Ok(named(REVIEWER_KEY).or_else(|| named("user.name")))
	}

	/// Remembers `name` as this repository's default reviewer, in its local
	/// git configuration, or forgets the remembered name when `None`.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::GitExecutable`] when `git` cannot run or
	/// fails.
	pub fn remember_reviewer(
		&self,
		name: Option<&str>,
	) -> Result<(), BackendError> {
		let args = match name {
			Some(name) => vec!["config", "--local", "--", REVIEWER_KEY, name],
			None => vec!["config", "--local", "--unset-all", REVIEWER_KEY],
		};
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => Ok(()),
			// Git exits with 5 when there is nothing to unset.
			Some(5) if name.is_none() => Ok(()),
			_ => Err(exit_failure(&args, &finished)),
		}
	}

	/// The working-tree files that git neither tracks nor ignores, sorted:
	/// `git ls-files --others --exclude-standard`. Snapshots list the index
	/// only, so `check` uses this to name what it leaves out.
	///
	/// A name that is not a canonical repository path is left out: an
	/// untracked file is never a source, so it cannot matter.
	///
	/// # Errors
	///
	/// Fails when `git` fails.
	pub fn untracked_files(&self) -> Result<Vec<RepoPath>, BackendError> {
		let output = self
			.git
			.output(&["ls-files", "--others", "--exclude-standard", "-z"], None)?;
		let mut paths: Vec<RepoPath> = output
			.split(|&byte| byte == 0)
			.filter(|raw| !raw.is_empty())
			.filter_map(|raw| crate::index::repo_path(raw).ok())
			.collect();
		paths.sort();
		Ok(paths)
	}

	/// The `git` executable bound to this repository.
	pub(crate) fn git(&self) -> &Git {
		&self.git
	}

	/// The git directory shared by all worktrees, which holds `info/` and
	/// `shallow`.
	pub(crate) fn common_dir(&self) -> &Path {
		&self.common_dir
	}

	/// Converts an ID for a `git2` lookup, or `None` when it uses another
	/// hash algorithm than this repository.
	pub(crate) fn git_oid(&self, id: &ObjectId) -> Option<git2::Oid> {
		git_oid(id, self.format)
	}

	/// The underlying `git2` repository.
	pub(crate) fn raw(&self) -> &git2::Repository {
		&self.repository
	}

	/// The working tree.
	///
	/// # Errors
	///
	/// Fails for a bare repository.
	pub(crate) fn require_work_dir(&self) -> Result<&Path, BackendError> {
		self.work_dir().ok_or_else(|| {
			failure(
				BackendErrorKind::Git,
				"the repository is bare, so it has no working tree",
			)
		})
	}

	/// A blob's content, or `None` when the object database has no blob with
	/// this ID, including when the ID names another kind of object.
	///
	/// # Errors
	///
	/// Fails when the object database cannot be read.
	pub(crate) fn blob_content(
		&self,
		id: &ObjectId,
	) -> Result<Option<Vec<u8>>, BackendError> {
		let Some(oid) = self.git_oid(id) else {
			return Ok(None);
		};
		match self.repository.find_blob(oid) {
			Ok(blob) => Ok(Some(blob.content().to_vec())),
			Err(source) if source.code() == git2::ErrorCode::NotFound => Ok(None),
			Err(source) => {
				Err(error::git(format!("cannot read the blob {id}"), source))
			}
		}
	}

	/// A blob's content that the snapshot needs, for the file at `path`.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::MissingObject`] when the blob is
	/// missing.
	pub(crate) fn required_blob(
		&self,
		id: &ObjectId,
		path: &RepoPath,
	) -> Result<Vec<u8>, BackendError> {
		self.blob_content(id)?.ok_or_else(|| {
			failure(
				BackendErrorKind::MissingObject,
				format!("the blob {id} of {path} is missing from the object database"),
			)
		})
	}

	/// A boolean git setting, `false` when unset.
	///
	/// # Errors
	///
	/// Fails when `git` fails, including for a value that is not a boolean.
	pub(crate) fn config_bool(&self, key: &str) -> Result<bool, BackendError> {
		let args = ["config", "--type=bool", "--get", key];
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => Ok(finished.stdout.trim_ascii() == b"true"),
			// Git exits with 1 when the key is unset.
			Some(1) => Ok(false),
			_ => Err(exit_failure(&args, &finished)),
		}
	}

	/// A boolean git setting, `false` when unset or unreadable. Used only for
	/// diagnostics.
	pub(crate) fn config_flag(&self, key: &str) -> bool {
		self
			.git
			.output(&["config", "--type=bool", "--get", key], None)
			.is_ok_and(|output| output.trim_ascii() == b"true")
	}

	/// A path that `git var` reports, or `None` when git has none. Used only
	/// for diagnostics.
	pub(crate) fn git_var(&self, name: &str) -> Option<PathBuf> {
		let output = self.git.output(&["var", name], None).ok()?;
		let text = String::from_utf8(output).ok()?;
		let text = text.trim_end_matches(['\r', '\n']);
		(!text.is_empty()).then(|| PathBuf::from(text))
	}
}

/// The key under which git's configuration remembers the name to record.
const REVIEWER_KEY: &str = "source-sentry.reviewer";
