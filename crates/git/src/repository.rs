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
	prefix: Option<RepoPath>,
	index_file: Option<PathBuf>,
	command_style: Option<Box<CommandStyle>>,
}

/// Writes a git command that a message advises the user to run: given its
/// arguments and the repository paths it names, it returns the command line.
///
/// A program that runs git elsewhere than in the working tree's root, such
/// as after `-C`, sets one with [`GitRepository::set_command_style`], so that
/// the advice runs as written where the user is.
pub type CommandStyle = dyn Fn(&[&str], &[&RepoPath]) -> String + Send + Sync;

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
	/// repository. Git's environment variables, such as `GIT_DIR`, are
	/// ignored, so that a library caller opens exactly the repository it
	/// names; [`GitRepository::open_from_env`] honours them.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::Git`] when no repository contains
	/// `path`, or when git refuses it, for example because another user owns
	/// it.
	pub fn open(path: impl AsRef<Path>) -> Result<Self, BackendError> {
		let path = path.as_ref();
		let repository = git2::Repository::discover(path)
			.map_err(|source| open_failure(path, source))?;
		let work_dir = repository.workdir().map(Path::to_path_buf);
		let common_dir = repository.commondir().to_path_buf();
		Self::from_parts(repository, work_dir, common_dir, None)
	}

	/// Opens the repository that `git` would use for a command started in
	/// `start`, with this process's environment, as the command line does.
	///
	/// Git itself is asked, so its environment variables apply as git
	/// applies them: `GIT_DIR` and `GIT_WORK_TREE`, `GIT_CEILING_DIRECTORIES`
	/// and `GIT_DISCOVERY_ACROSS_FILESYSTEM` while searching, a working tree
	/// that `core.worktree` names, and `safe.directory` from every level of
	/// git's configuration. A bare repository opens without a working tree,
	/// unless `GIT_WORK_TREE` names one.
	///
	/// Objects are read from the object directories that git reads,
	/// including those that `GIT_OBJECT_DIRECTORY` and
	/// `GIT_ALTERNATE_OBJECT_DIRECTORIES` name. `GIT_COMMON_DIR` is followed
	/// only to the common directory that the git directory names itself,
	/// because `git2` cannot be pointed at another. The `git` commands that
	/// read or write the index use the one that `GIT_INDEX_FILE` names, a
	/// relative one taken from the working tree's root.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::Git`] when git finds no repository or
	/// refuses it, when `start` lies inside a git directory, when the working
	/// tree that git reports is not a directory, and when `git2` cannot open
	/// the same repository; with [`BackendErrorKind::GitExecutable`] when
	/// `git` cannot run.
	pub fn open_from_env(start: impl AsRef<Path>) -> Result<Self, BackendError> {
		let start = start.as_ref();
		let location = crate::discover::locate(start)?;
		// Git applied its environment above. `git2` opens exactly the
		// directory git reported, without reading the environment, whose
		// handling in libgit2 differs from git's.
		let repository = git2::Repository::open_ext(
			&location.git_dir,
			git2::RepositoryOpenFlags::NO_SEARCH
				| git2::RepositoryOpenFlags::NO_DOTGIT,
			std::iter::empty::<&std::ffi::OsStr>(),
		)
		.map_err(|source| open_failure(&location.git_dir, source))?;
		same_directory(repository.path(), &location.git_dir, || {
			format!(
				"git2 opened the git directory {} where git uses {}",
				repository.path().display(),
				location.git_dir.display()
			)
		})?;
		// `git2` reads the common directory that the git directory names
		// itself, and cannot be pointed at another one.
		same_directory(repository.commondir(), &location.common_dir, || {
			if std::env::var_os("GIT_COMMON_DIR").is_some() {
				format!(
					"GIT_COMMON_DIR names the common git directory {}, but Source \
					 Sentry can read only the one that the git directory {} names \
					 itself, {}",
					location.common_dir.display(),
					location.git_dir.display(),
					repository.commondir().display()
				)
			} else {
				format!(
					"git2 reads the common git directory {} where git uses {}",
					repository.commondir().display(),
					location.common_dir.display()
				)
			}
		})?;
		let alternates = crate::discover::alternates(start)?;
		attach_objects(
			&repository,
			&location.objects,
			alternates.as_deref().unwrap_or_default(),
		)?;
		let mut opened = Self::from_parts(
			repository,
			location.work_dir,
			location.common_dir,
			location.prefix,
		)?;
		// Git's own commands run in the working tree, not the start
		// directory, so they get the directories as git resolved them.
		if std::env::var_os("GIT_COMMON_DIR").is_some() {
			let common = opened.common_dir.clone().into_os_string();
			opened.git.redirect("GIT_COMMON_DIR", common);
		}
		if std::env::var_os("GIT_OBJECT_DIRECTORY").is_some() {
			opened
				.git
				.redirect("GIT_OBJECT_DIRECTORY", location.objects.into_os_string());
		}
		if let Some(alternates) = alternates {
			opened.git.redirect(
				"GIT_ALTERNATE_OBJECT_DIRECTORIES",
				crate::discover::join_alternates(&alternates)?,
			);
		}
		// Git's commands that read or write the index take a relative
		// `GIT_INDEX_FILE` from the working tree's root, which is where a
		// `pre-commit` hook gets `.git/index` from. An empty value is passed
		// on for git to refuse.
		if let Some(index) = std::env::var_os("GIT_INDEX_FILE") {
			let index = if index.is_empty() {
				PathBuf::from(index)
			} else {
				let base = opened
					.work_dir
					.as_deref()
					.unwrap_or_else(|| opened.repository.path());
				base.join(index)
			};
			opened
				.git
				.redirect("GIT_INDEX_FILE", index.clone().into_os_string());
			opened.index_file = Some(index);
		}
		Ok(opened)
	}

	/// A repository that `git2` opened, with its working tree and common
	/// directory.
	fn from_parts(
		repository: git2::Repository,
		work_dir: Option<PathBuf>,
		common_dir: PathBuf,
		prefix: Option<RepoPath>,
	) -> Result<Self, BackendError> {
		let format = repository.object_format();
		let algorithm = algorithm_of_format(format)?;
		let git = Git::new(repository.path(), work_dir.as_deref());
		Ok(Self {
			repository,
			git,
			format,
			algorithm,
			work_dir,
			common_dir,
			prefix,
			index_file: None,
			command_style: None,
		})
	}

	/// Sets how messages write the git commands they advise. Without one,
	/// a command is written to run in the working tree's root: `git`, the
	/// arguments, `--` and the repository paths.
	pub fn set_command_style(
		&mut self,
		style: impl Fn(&[&str], &[&RepoPath]) -> String + Send + Sync + 'static,
	) {
		self.command_style = Some(Box::new(style));
	}

	/// A git command for a message, written in the style that
	/// [`GitRepository::set_command_style`] set.
	pub(crate) fn advised_command(
		&self,
		arguments: &[&str],
		paths: &[&RepoPath],
	) -> String {
		if let Some(style) = &self.command_style {
			return style(arguments, paths);
		}
		let mut words = vec!["git"];
		words.extend(arguments);
		words.push("--");
		words.extend(paths.iter().map(|path| path.as_str()));
		words.join(" ")
	}

	/// Where the directory that [`GitRepository::open_from_env`] started in
	/// lies in the working tree, as `git rev-parse --show-prefix` reports
	/// it: the root itself at the top. `None` when it lies outside the
	/// working tree, which `GIT_WORK_TREE` allows, when the repository is
	/// bare, and for a repository opened with [`GitRepository::open`].
	#[must_use]
	pub fn prefix(&self) -> Option<&RepoPath> {
		self.prefix.as_ref()
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
	/// honours applies. Each key has the value git uses, the one set last;
	/// when that value is empty, the key counts as unset, even if another
	/// level sets a name.
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
		// Each entry is `<key>\n<value>`, or `<key>` alone for a key without
		// a value, and ends with a NUL. A key set at several levels is listed
		// once for each; git uses the last, and only that value decides
		// whether a name is set.
		let named = |key: &str| {
			listed
				.split('\0')
				.filter(|entry| !entry.is_empty())
				.map(|entry| entry.split_once('\n').unwrap_or((entry, "")))
				.filter(|(name, _)| *name == key)
				.map(|(_, value)| value)
				.next_back()
				.filter(|value| !value.trim().is_empty())
				.map(str::to_owned)
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

	/// The value of a git setting as git reads it, with every level and
	/// include it honours, or `None` when no configuration sets it.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::GitExecutable`] when `git` cannot run or
	/// fails, and with [`BackendErrorKind::Git`] when the value is not UTF-8.
	pub fn config_value(
		&self,
		key: &str,
	) -> Result<Option<String>, BackendError> {
		let args = ["config", "--get", key];
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => {}
			// Git exits with 1 when the key is unset.
			Some(1) => return Ok(None),
			_ => return Err(exit_failure(&args, &finished)),
		}
		let mut value = String::from_utf8(finished.stdout).map_err(|source| {
			caused(
				BackendErrorKind::Git,
				format!("git's `{key}` is not UTF-8"),
				source,
			)
		})?;
		if value.ends_with('\n') {
			value.pop();
		}
		Ok(Some(value))
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

	/// The index file that `GIT_INDEX_FILE` names, when git is committing
	/// with a temporary one: a lock file other than the repository's own
	/// index, which git sets for a `pre-commit` hook of `git commit -a` or
	/// `git commit <paths>`, and removes when the commit ends.
	pub(crate) fn temporary_index(&self) -> Option<&Path> {
		let index = self.index_file.as_deref()?;
		let lock = index
			.file_name()
			.is_some_and(|name| name.to_string_lossy().ends_with(".lock"));
		let own = self.repository.path().join("index");
		let same = index == own
			|| index
				.canonicalize()
				.is_ok_and(|index| own.canonicalize().is_ok_and(|own| index == own));
		(lock && !same).then_some(index)
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
		self.config_bool_or(key, false)
	}

	/// Whether the working tree holds symbolic links as plain files that hold
	/// their target: the effective `core.symlinks` is false. Unset, it is
	/// false with Git for Windows and true elsewhere, as git itself defaults
	/// it.
	///
	/// # Errors
	///
	/// Fails when `git` fails, including for a value that is not a boolean.
	pub(crate) fn symlink_placeholders(&self) -> Result<bool, BackendError> {
		Ok(!self.config_bool_or("core.symlinks", !cfg!(windows))?)
	}

	/// A boolean git setting, `unset` when no configuration file sets it.
	///
	/// # Errors
	///
	/// Fails when `git` fails, including for a value that is not a boolean.
	pub(crate) fn config_bool_or(
		&self,
		key: &str,
		unset: bool,
	) -> Result<bool, BackendError> {
		let args = ["config", "--type=bool", "--get", key];
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => Ok(finished.stdout.trim_ascii() == b"true"),
			// Git exits with 1 when the key is unset.
			Some(1) => Ok(unset),
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

	/// A path that `git var` reports, or `None` when git has none.
	///
	/// # Errors
	///
	/// Fails when `git` cannot run, fails otherwise than by having no such
	/// path, or prints a path that is not UTF-8, so that a path git has is
	/// never mistaken for none.
	pub(crate) fn git_var(
		&self,
		name: &str,
	) -> Result<Option<PathBuf>, BackendError> {
		let args = ["var", name];
		let finished = self.git.run(&args, None)?;
		match finished.status.code() {
			Some(0) => {}
			// Git exits with 1 when it has no such path, as for the system
			// attribute file under `GIT_ATTR_NOSYSTEM`.
			Some(1) => return Ok(None),
			_ => return Err(exit_failure(&args, &finished)),
		}
		let text = String::from_utf8(finished.stdout).map_err(|source| {
			caused(
				BackendErrorKind::Git,
				format!("`git var {name}` printed a path that is not UTF-8"),
				source,
			)
		})?;
		let text = text.trim_end_matches(['\r', '\n']);
		Ok((!text.is_empty()).then(|| PathBuf::from(text)))
	}
}

/// Gives `repository` the object database git uses: the object directory
/// it reported, which `GIT_OBJECT_DIRECTORY` may name, with the alternates
/// that directory lists, and the alternates of the environment.
///
/// # Errors
///
/// Fails with [`BackendErrorKind::Git`] when a directory is not UTF-8,
/// which `git2` cannot take, or cannot be read.
fn attach_objects(
	repository: &git2::Repository,
	objects: &Path,
	alternates: &[PathBuf],
) -> Result<(), BackendError> {
	let database = git2::Odb::new_ext(repository.object_format())
		.map_err(|source| error::git("cannot create an object database", source))?;
	for directory in
		std::iter::once(objects).chain(alternates.iter().map(PathBuf::as_path))
	{
		let text = directory.to_str().ok_or_else(|| {
			failure(
				BackendErrorKind::Git,
				format!(
					"cannot read the object directory {}: its path is not UTF-8",
					directory.display()
				),
			)
		})?;
		database.add_disk_alternate(text).map_err(|source| {
			error::git(
				format!("cannot read the object directory {}", directory.display()),
				source,
			)
		})?;
	}
	repository
		.set_odb(&database)
		.map_err(|source| error::git("cannot use the object database", source))
}

/// A repository that `git2` could not open at `path`.
fn open_failure(path: &Path, source: git2::Error) -> BackendError {
	let hint = if source.message().contains("object format") {
		"; reading SHA-256 repositories needs git2's `unstable-sha256` feature"
	} else {
		""
	};
	caused(
		BackendErrorKind::Git,
		format!("cannot open a git repository at {}{hint}", path.display()),
		source,
	)
}

/// Checks that `git2` and git name the same directory, comparing the
/// directories as the filesystem resolves them, since the two spell paths
/// differently.
///
/// # Errors
///
/// Fails with [`BackendErrorKind::Git`] and `message` when they differ or
/// either cannot be resolved.
fn same_directory(
	opened: &Path,
	reported: &Path,
	message: impl FnOnce() -> String,
) -> Result<(), BackendError> {
	match (opened.canonicalize(), reported.canonicalize()) {
		(Ok(opened), Ok(reported)) if opened == reported => Ok(()),
		_ => Err(failure(BackendErrorKind::Git, message())),
	}
}

/// The key under which git's configuration remembers the name to record.
const REVIEWER_KEY: &str = "source-sentry.reviewer";
