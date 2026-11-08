//! Applies write plans: the only code in Source Sentry that writes or deletes
//! files, or changes the index.
//!
//! A plan is applied in five phases. Nothing visible changes before the
//! third, and the third and fourth undo themselves if any of their steps
//! fails:
//!
//! 1. **Verify.** Every target must still have the canonical oid the plan
//!    expects, computed exactly as a working-tree snapshot computes it. A
//!    write whose expectation is `None` requires that no file exists; a
//!    delete requires the file. Symbolic links, paths below them,
//!    sparse-checkout paths, submodules and non-files are refused, and so is
//!    a new file that git ignores or that lies outside the sparse-checkout
//!    definition, because neither could be marked intent-to-add.
//! 2. **Stage.** Each new content is written to a temporary file beside its
//!    target and flushed to disk, and missing parent directories are created.
//!    Beside each file to delete, an empty placeholder reserves a unique
//!    backup name.
//! 3. **Commit.** Each file to delete is renamed onto its backup name, then
//!    each staged file is renamed over its target. If a rename fails, the
//!    steps already taken are undone in reverse: backups are renamed back,
//!    replaced files are restored from the contents read during
//!    verification, and new files are removed.
//! 4. **Mark.** The files the plan creates, those the index did not hold
//!    before, are marked intent-to-add in one `git add --intent-to-add`, so
//!    the next snapshot sees them. If that fails, any path it
//!    recorded is removed from the index again and every step of the commit
//!    is undone, so the plan stays all or nothing. Rewritten and
//!    deleted files leave the index alone.
//! 5. **Clean up.** The backups of deleted files are removed. Directories
//!    that a delete leaves empty stay, because git does not track directories
//!    and one may hold meaning for the user.

use std::{
	collections::{BTreeMap, HashSet},
	fs::{self, File, OpenOptions},
	io::{self, Write as _},
	path::{Path, PathBuf},
	process,
};

use source_sentry_core::ports::{
	BackendError, Canonical, ObjectId, PlannedDelete, PlannedWrite, RepoPath,
	WritePlan,
};

use crate::{
	canonical::Canonicalizer,
	error,
	index::{IndexRecord, read_index},
	intent,
	presence::{is_absence, native},
	repository::GitRepository,
};

/// Why a write plan was not applied, or not applied cleanly.
///
/// Every variant except [`ApplyError::RollbackFailed`] guarantees that no file
/// was changed.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
	/// The plan names the same path more than once: twice among its writes,
	/// twice among its deletes, or once in each.
	#[error("the write plan names {0} more than once")]
	DuplicatePath(RepoPath),
	/// A target cannot be written or deleted safely.
	#[error("cannot change {path}: {reason}")]
	Unwritable {
		/// The target.
		path: RepoPath,
		/// Why it cannot be changed.
		reason: Unwritable,
	},
	/// A target's canonical oid is not the one the plan expects, so the file
	/// changed after the plan was made. For a delete, `expected` is always
	/// `Some`, and `actual` is `None` when the file is missing.
	#[error("{path} changed since the plan was made: expected {}, found {}", describe(expected.as_ref()), describe(actual.as_ref()))]
	Stale {
		/// The target.
		path: RepoPath,
		/// The oid the plan expects, or `None` for a file that must not
		/// exist.
		expected: Option<ObjectId>,
		/// The file's canonical oid, or `None` when it does not exist.
		actual: Option<ObjectId>,
	},
	/// A target's current content cannot be canonicalized, so the plan's
	/// expectation cannot be checked.
	#[error("cannot verify {path}: {}", describe_reason(reason))]
	Unverifiable {
		/// The target.
		path: RepoPath,
		/// Why no canonical oid could be computed.
		reason: Canonical,
	},
	/// A target changed while the plan was being verified.
	#[error("{0} changed while the write plan was being verified")]
	ChangedDuringVerification(RepoPath),
	/// The backend failed before anything was changed.
	#[error(transparent)]
	Backend(#[from] BackendError),
	/// Writing a file failed. Every change already made was undone, so
	/// nothing changed.
	#[error("cannot write {path}; nothing was changed")]
	WriteFailed {
		/// The target that could not be written.
		path: RepoPath,
		/// The underlying failure.
		#[source]
		source: io::Error,
	},
	/// Marking the created files intent-to-add failed. Every
	/// change already made was undone, the index included, so nothing
	/// changed; the plan can be applied again once the cause, such as another
	/// git process holding `index.lock`, is gone.
	#[error("cannot mark {} intent-to-add; nothing was changed", list(paths))]
	IntentToAddFailed {
		/// The files the plan would have created.
		paths: Vec<RepoPath>,
		/// Why `git add --intent-to-add` failed.
		#[source]
		source: BackendError,
	},
	/// Deleting a file failed. Every change already made was undone, so
	/// nothing changed.
	#[error("cannot delete {path}; nothing was changed")]
	DeleteFailed {
		/// The file that could not be deleted.
		path: RepoPath,
		/// The underlying failure.
		#[source]
		source: io::Error,
	},
	/// Applying the plan failed, and so did undoing some of the changes
	/// already made. A written file in `unrestored` may hold its new
	/// contents, or be missing, and a created one may remain marked
	/// intent-to-add. A deleted file in `unrestored` is missing, and its
	/// contents remain beside it in a hidden file named
	/// `.<name>.<process>.<n>.source-sentry.bak`. When marking failed, `path`
	/// is the first created file, and `source` wraps the [`BackendError`].
	#[error("cannot change {path}, and cannot restore {}", list(unrestored))]
	RollbackFailed {
		/// The target whose change failed.
		path: RepoPath,
		/// The underlying failure.
		#[source]
		source: io::Error,
		/// The files that could not be restored.
		unrestored: Vec<RepoPath>,
	},
}

/// Why a target cannot be written or deleted safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unwritable {
	/// The target, or a directory above it, is a symbolic link, which Source
	/// Sentry never follows.
	#[error("it is or lies below a symbolic link")]
	SymbolicLink,
	/// The target is a directory or another non-file, or a component above it
	/// is a file.
	#[error("it is not a regular file")]
	NotAFile,
	/// Sparse checkout excludes the path: it is marked skip-worktree, so git
	/// would ignore a change there, or it is a new file outside the
	/// sparse-checkout definition, which git refuses to mark intent-to-add.
	#[error("sparse checkout excludes it from the working tree")]
	SparseCheckout,
	/// The path is a new file that git ignores, through `.gitignore`,
	/// `info/exclude` or `core.excludesFile`. It could not be marked
	/// intent-to-add without overriding that rule, and a file git never
	/// commits would leave CI's evaluation different from the local one.
	#[error("git ignores it, so it could not be marked intent-to-add")]
	Ignored,
	/// The path is a submodule or lies inside one. Submodules are
	/// unsupported, and the superproject does not track their
	/// files.
	#[error("it is or lies inside a submodule")]
	Submodule,
	/// The path is the repository root.
	#[error("it is the repository root")]
	Root,
}

/// Writes `ObjectId`s and absence for messages.
fn describe(id: Option<&ObjectId>) -> String {
	id.map_or_else(|| "no file".to_owned(), ToString::to_string)
}

/// Explains a [`Canonical`] that is not an oid.
fn describe_reason(reason: &Canonical) -> String {
	match reason {
		Canonical::Oid(id) => id.to_string(),
		Canonical::UnsupportedFilter {
			filter,
			attribute_origin,
		} => format!("unsupported clean filter {filter} ({attribute_origin})"),
		Canonical::UnsupportedLfsExtension => {
			"a Git LFS pointer uses extensions".to_owned()
		}
		Canonical::CrlfInIndex => "the index holds it with CRLF line endings \
		                           although its attributes ask git to \
		                           normalize it; run `git add --renormalize .`"
			.to_owned(),
		Canonical::Submodule => "it is a submodule".to_owned(),
	}
}

/// Joins paths for a message.
fn list(paths: &[RepoPath]) -> String {
	paths
		.iter()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join(", ")
}

impl GitRepository {
	/// Applies `plan` to the working tree, all or nothing.
	///
	/// Each target must still have its expected canonical oid, computed as a
	/// working-tree snapshot computes it. A write whose expectation is `None`
	/// requires that no file exists, and a delete requires the file. Only then
	/// is anything changed: each new content goes to a temporary file beside
	/// its target and is renamed into place, and each deleted file is first
	/// renamed to a backup beside it, which is removed once every change has
	/// succeeded. Directories that a delete leaves empty stay.
	///
	/// Files the plan creates, those the index did not hold before, are then
	/// marked intent-to-add, so the next snapshot sees them; this is the only
	/// change Source Sentry makes to the index. Files the plan
	/// rewrites or deletes leave the index alone.
	///
	/// # Errors
	///
	/// Fails without changing anything when the plan is inconsistent, a
	/// target is unsafe to change, an expectation does not hold, or a change
	/// or the marking fails and every change already made is undone. Fails
	/// with
	/// [`ApplyError::RollbackFailed`] in the one case where files may be left
	/// changed: a change failed, and undoing an earlier one failed too.
	pub fn apply(&self, plan: &WritePlan) -> Result<(), ApplyError> {
		if plan.writes.is_empty() && plan.deletes.is_empty() {
			return Ok(());
		}
		let work_dir = self.require_work_dir()?;
		check_paths(plan)?;

		let index = read_index(self.git(), self.algorithm())?;
		let changed = plan
			.writes
			.iter()
			.map(|write| &write.path)
			.chain(plan.deletes.iter().map(|delete| &delete.path));
		for path in changed {
			if index.get(path).is_some_and(|record| record.skip_worktree) {
				return Err(unwritable(path, Unwritable::SparseCheckout));
			}
			if inside_submodule(&index, path) {
				return Err(unwritable(path, Unwritable::Submodule));
			}
		}
		let writes = plan
			.writes
			.iter()
			.map(|write| {
				inspect(work_dir, &write.path)
					.map(|inspected| WriteTarget { write, inspected })
			})
			.collect::<Result<Vec<_>, _>>()?;
		let deletes = plan
			.deletes
			.iter()
			.map(|delete| inspect_delete(work_dir, delete))
			.collect::<Result<Vec<_>, _>>()?;
		let created: Vec<RepoPath> = writes
			.iter()
			.filter(|target| {
				target.inspected.original.is_none()
					&& !index.contains_key(&target.write.path)
			})
			.map(|target| target.write.path.clone())
			.collect();
		self.check_markable(&created)?;
		self.verify(&index, &writes, &deletes)?;

		let mut created_directories = Vec::new();
		let mut steps = match stage(writes, deletes, &mut created_directories) {
			Ok(steps) => steps,
			Err(failure) => {
				remove_directories(&created_directories);
				return Err(failure.into_error());
			}
		};
		let result = commit(&mut steps, &mut |from, to| fs::rename(from, to));
		let Err((failed, source)) = result else {
			if created.is_empty() {
				// Dropping the steps removes the backups of deleted files.
				return Ok(());
			}
			return self.mark_created(&created, steps, &created_directories);
		};
		let failure = Failure {
			path: steps[failed].path().clone(),
			deleting: matches!(steps[failed], Step::Delete { .. }),
			source,
		};
		let unrestored = roll_back(&mut steps[..failed]);
		drop(steps);
		remove_directories(&created_directories);
		if unrestored.is_empty() {
			Err(failure.into_error())
		} else {
			Err(ApplyError::RollbackFailed {
				path: failure.path,
				source: failure.source,
				unrestored,
			})
		}
	}

	/// Refuses new files that `git add --intent-to-add` would not record:
	/// ignored paths, and paths outside the sparse-checkout definition. Git
	/// records the other paths and still fails for those, so they must be
	/// found before anything is written.
	fn check_markable(&self, created: &[RepoPath]) -> Result<(), ApplyError> {
		if created.is_empty() {
			return Ok(());
		}
		if self.config_bool("core.sparseCheckout")?
			&& let Some(path) =
				intent::outside_sparse_checkout(self.git(), created)?.first()
		{
			return Err(unwritable(path, Unwritable::SparseCheckout));
		}
		if let Some(path) = intent::ignored(self.git(), created)?.first() {
			return Err(unwritable(path, Unwritable::Ignored));
		}
		Ok(())
	}

	/// Marks the created files intent-to-add once every step has been taken.
	/// If that fails, removes from the index any path it recorded, and undoes
	/// every step.
	fn mark_created(
		&self,
		created: &[RepoPath],
		mut steps: Vec<Step>,
		created_directories: &[PathBuf],
	) -> Result<(), ApplyError> {
		let Err(error) = intent::mark(self.git(), created) else {
			// Dropping the steps removes the backups of deleted files.
			return Ok(());
		};
		// A failed `git add` normally records nothing, but whatever it did
		// record must go before the files do.
		let recorded: Vec<RepoPath> = match read_index(self.git(), self.algorithm())
		{
			Ok(index) => created
				.iter()
				.filter(|path| index.contains_key(*path))
				.cloned()
				.collect(),
			Err(_) => created.to_vec(),
		};
		let mut unrestored = Vec::new();
		if !recorded.is_empty() && intent::unmark(self.git(), &recorded).is_err() {
			unrestored.extend(recorded);
		}
		for path in roll_back(&mut steps) {
			if !unrestored.contains(&path) {
				unrestored.push(path);
			}
		}
		drop(steps);
		remove_directories(created_directories);
		if unrestored.is_empty() {
			return Err(ApplyError::IntentToAddFailed {
				paths: created.to_vec(),
				source: error,
			});
		}
		Err(ApplyError::RollbackFailed {
			path: created.first().cloned().unwrap_or_default(),
			source: io::Error::other(error),
			unrestored,
		})
	}

	/// Checks every target against the plan's expectation, computing all
	/// canonical oids in one batch.
	fn verify(
		&self,
		index: &BTreeMap<RepoPath, IndexRecord>,
		writes: &[WriteTarget<'_>],
		deletes: &[DeleteTarget<'_>],
	) -> Result<(), ApplyError> {
		let existing: Vec<&RepoPath> = writes
			.iter()
			.filter(|target| target.inspected.original.is_some())
			.map(|target| &target.write.path)
			.chain(deletes.iter().map(|target| &target.delete.path))
			.collect();
		let mut canonical = Canonicalizer::new(self, index)
			.regular_files(&existing)?
			.into_iter();
		let mut next_oid = |path: &RepoPath| match canonical.next() {
			Some(Canonical::Oid(id)) => Ok(id),
			Some(reason) => Err(ApplyError::Unverifiable {
				path: path.clone(),
				reason,
			}),
			None => Err(ApplyError::ChangedDuringVerification(path.clone())),
		};
		for target in writes {
			let write = target.write;
			let actual = match &target.inspected.original {
				None => None,
				Some(_) => Some(next_oid(&write.path)?),
			};
			if actual != write.expected {
				return Err(ApplyError::Stale {
					path: write.path.clone(),
					expected: write.expected.clone(),
					actual,
				});
			}
		}
		for target in deletes {
			let delete = target.delete;
			let actual = next_oid(&delete.path)?;
			if actual != delete.expected {
				return Err(ApplyError::Stale {
					path: delete.path.clone(),
					expected: Some(delete.expected.clone()),
					actual: Some(actual),
				});
			}
		}
		// The oids were computed from the files on disk; the contents kept for
		// rollback must be the same files.
		let originals =
			writes
				.iter()
				.filter_map(|target| {
					let original = target.inspected.original.as_ref()?;
					Some((&target.write.path, &target.inspected.native, original))
				})
				.chain(deletes.iter().map(|target| {
					(&target.delete.path, &target.native, &target.original)
				}));
		for (path, native, original) in originals {
			if fs::read(native).ok().as_deref() != Some(original.contents.as_slice())
			{
				return Err(ApplyError::ChangedDuringVerification(path.clone()));
			}
		}
		Ok(())
	}
}

/// Refuses the root, and any path the plan names more than once.
fn check_paths(plan: &WritePlan) -> Result<(), ApplyError> {
	let mut seen = HashSet::new();
	let paths = plan
		.writes
		.iter()
		.map(|write| &write.path)
		.chain(plan.deletes.iter().map(|delete| &delete.path));
	for path in paths {
		if path.is_root() {
			return Err(unwritable(path, Unwritable::Root));
		}
		if !seen.insert(path) {
			return Err(ApplyError::DuplicatePath(path.clone()));
		}
	}
	Ok(())
}

/// Whether `path` or a directory above it is a submodule (gitlink) in the
/// index.
fn inside_submodule(
	index: &BTreeMap<RepoPath, IndexRecord>,
	path: &RepoPath,
) -> bool {
	let mut current = Some(path.clone());
	while let Some(candidate) = current {
		if index.get(&candidate).is_some_and(IndexRecord::is_gitlink) {
			return true;
		}
		current = candidate.parent();
	}
	false
}

/// An [`ApplyError::Unwritable`] for `path`.
fn unwritable(path: &RepoPath, reason: Unwritable) -> ApplyError {
	ApplyError::Unwritable {
		path: path.clone(),
		reason,
	}
}

/// What is on disk at a target now.
struct Inspected {
	/// The native path of the target.
	native: PathBuf,
	/// The directories above the target that do not exist yet, outermost
	/// first.
	missing_directories: Vec<PathBuf>,
	/// The current file, or `None` when there is none.
	original: Option<Original>,
}

/// One planned write, with what is on disk now.
struct WriteTarget<'plan> {
	write: &'plan PlannedWrite,
	inspected: Inspected,
}

/// One planned delete, with the file it removes.
struct DeleteTarget<'plan> {
	delete: &'plan PlannedDelete,
	/// The native path of the file.
	native: PathBuf,
	/// The file, which a delete requires.
	original: Original,
}

/// A file's contents and permissions before the plan, kept for rollback and
/// for detecting a change during verification.
struct Original {
	contents: Vec<u8>,
	permissions: fs::Permissions,
}

/// Inspects a target without following symbolic links.
fn inspect(work_dir: &Path, path: &RepoPath) -> Result<Inspected, ApplyError> {
	let mut missing_directories = Vec::new();
	let mut directory = work_dir.to_path_buf();
	let components: Vec<&str> = path.components().collect();
	let (_, parents) = components.split_last().unwrap_or((&"", &[]));
	for component in parents {
		directory.push(component);
		if !missing_directories.is_empty() {
			missing_directories.push(directory.clone());
			continue;
		}
		match fs::symlink_metadata(&directory) {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				return Err(unwritable(path, Unwritable::SymbolicLink));
			}
			Ok(metadata) if metadata.is_dir() => {}
			Ok(_) => return Err(unwritable(path, Unwritable::NotAFile)),
			Err(source) if is_absence(&source) => {
				missing_directories.push(directory.clone());
			}
			Err(source) => {
				return Err(error::io(format!("cannot inspect {path}"), source).into());
			}
		}
	}
	let native = native(work_dir, path);
	let original = if missing_directories.is_empty() {
		match fs::symlink_metadata(&native) {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				return Err(unwritable(path, Unwritable::SymbolicLink));
			}
			Ok(metadata) if metadata.is_file() => {
				let contents = fs::read(&native).map_err(|source| {
					ApplyError::from(error::io(format!("cannot read {path}"), source))
				})?;
				Some(Original {
					contents,
					permissions: metadata.permissions(),
				})
			}
			Ok(_) => return Err(unwritable(path, Unwritable::NotAFile)),
			Err(source) if is_absence(&source) => None,
			Err(source) => {
				return Err(error::io(format!("cannot inspect {path}"), source).into());
			}
		}
	} else {
		None
	};
	Ok(Inspected {
		native,
		missing_directories,
		original,
	})
}

/// Inspects the file a delete removes, which must exist.
fn inspect_delete<'plan>(
	work_dir: &Path,
	delete: &'plan PlannedDelete,
) -> Result<DeleteTarget<'plan>, ApplyError> {
	let inspected = inspect(work_dir, &delete.path)?;
	let Some(original) = inspected.original else {
		return Err(ApplyError::Stale {
			path: delete.path.clone(),
			expected: Some(delete.expected.clone()),
			actual: None,
		});
	};
	Ok(DeleteTarget {
		delete,
		native: inspected.native,
		original,
	})
}

/// A temporary file beside a target, removed when dropped while armed.
///
/// It holds either a staged new content, which a successful rename moves
/// away, or the backup of a deleted file, which is removed once the plan has
/// succeeded.
struct Staged {
	path: PathBuf,
	armed: bool,
}

impl Staged {
	/// Writes `contents` to a new file beside `target`, named
	/// `.<name>.<process>.<n>.source-sentry.<suffix>`, with `permissions` on
	/// Unix, and flushes it to disk.
	fn create(
		target: &Path,
		contents: &[u8],
		permissions: Option<&fs::Permissions>,
		suffix: &str,
	) -> io::Result<Self> {
		let directory = target
			.parent()
			.ok_or_else(|| io::Error::other("the target has no parent directory"))?;
		let name = target
			.file_name()
			.ok_or_else(|| io::Error::other("the target has no file name"))?
			.to_string_lossy()
			.into_owned();
		let mut attempt = 0_u32;
		let (path, mut file) = loop {
			let candidate = directory.join(format!(
				".{name}.{}.{attempt}.source-sentry.{suffix}",
				process::id()
			));
			match OpenOptions::new()
				.write(true)
				.create_new(true)
				.open(&candidate)
			{
				Ok(file) => break (candidate, file),
				Err(source)
					if source.kind() == io::ErrorKind::AlreadyExists
						&& attempt < 1000 =>
				{
					attempt += 1;
				}
				Err(source) => return Err(source),
			}
		};
		let staged = Self { path, armed: true };
		write_and_sync(&mut file, contents)?;
		drop(file);
		// Only Unix permissions, such as an executable bit, are copied. A
		// read-only copy on Windows could not be cleaned up.
		#[cfg(unix)]
		if let Some(permissions) = permissions {
			fs::set_permissions(&staged.path, permissions.clone())?;
		}
		#[cfg(not(unix))]
		let _ = permissions;
		Ok(staged)
	}
}

impl Drop for Staged {
	fn drop(&mut self) {
		if self.armed {
			// Best effort: a leftover temporary file changes no tracked file.
			let _ = fs::remove_file(&self.path);
		}
	}
}

/// Writes all of `contents` and flushes them to disk.
fn write_and_sync(file: &mut File, contents: &[u8]) -> io::Result<()> {
	file.write_all(contents)?;
	file.sync_all()
}

/// One rename of the commit phase, with what undoing it needs.
enum Step {
	/// Moves a file to delete onto its backup, which stays armed so that it
	/// is removed once the plan has succeeded.
	Delete {
		path: RepoPath,
		target: PathBuf,
		backup: Staged,
	},
	/// Moves a staged content over its target.
	Write {
		path: RepoPath,
		target: PathBuf,
		staged: Staged,
		original: Option<Original>,
	},
}

impl Step {
	/// The repository path the step changes.
	fn path(&self) -> &RepoPath {
		match self {
			Self::Delete { path, .. } | Self::Write { path, .. } => path,
		}
	}
}

/// A change that failed, before the error that reports it is chosen.
struct Failure {
	path: RepoPath,
	deleting: bool,
	source: io::Error,
}

impl Failure {
	/// The error for a failure after which nothing changed.
	fn into_error(self) -> ApplyError {
		if self.deleting {
			ApplyError::DeleteFailed {
				path: self.path,
				source: self.source,
			}
		} else {
			ApplyError::WriteFailed {
				path: self.path,
				source: self.source,
			}
		}
	}
}

/// Creates missing directories, stages every new content, and reserves a
/// backup name beside every file to delete. Returns the commit steps, deletes
/// first. On failure, `created` lists the directories made so far, and the
/// files staged so far are removed when dropped.
fn stage(
	writes: Vec<WriteTarget<'_>>,
	deletes: Vec<DeleteTarget<'_>>,
	created: &mut Vec<PathBuf>,
) -> Result<Vec<Step>, Failure> {
	let mut steps = Vec::with_capacity(writes.len() + deletes.len());
	for target in deletes {
		let path = &target.delete.path;
		let backup =
			Staged::create(&target.native, b"", None, "bak").map_err(|source| {
				Failure {
					path: path.clone(),
					deleting: true,
					source,
				}
			})?;
		steps.push(Step::Delete {
			path: path.clone(),
			target: target.native,
			backup,
		});
	}
	for target in writes {
		let path = &target.write.path;
		let failed = |source| Failure {
			path: path.clone(),
			deleting: false,
			source,
		};
		for directory in &target.inspected.missing_directories {
			match fs::create_dir(directory) {
				Ok(()) => created.push(directory.clone()),
				// An earlier target in the plan created it.
				Err(source)
					if source.kind() == io::ErrorKind::AlreadyExists
						&& directory.is_dir() => {}
				Err(source) => return Err(failed(source)),
			}
		}
		let permissions = target
			.inspected
			.original
			.as_ref()
			.map(|original| &original.permissions);
		let staged = Staged::create(
			&target.inspected.native,
			&target.write.contents,
			permissions,
			"tmp",
		)
		.map_err(failed)?;
		steps.push(Step::Write {
			path: path.clone(),
			target: target.inspected.native,
			staged,
			original: target.inspected.original,
		});
	}
	Ok(steps)
}

/// Takes every step in order: moves each file to delete onto its backup, and
/// each staged content over its target. On failure, returns the index of the
/// step that failed; those before it have been taken.
fn commit(
	steps: &mut [Step],
	rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), (usize, io::Error)> {
	for (index, step) in steps.iter_mut().enumerate() {
		match step {
			Step::Delete { target, backup, .. } => {
				rename(target, &backup.path).map_err(|source| (index, source))?;
			}
			Step::Write { target, staged, .. } => {
				rename(&staged.path, target).map_err(|source| (index, source))?;
				staged.armed = false;
			}
		}
	}
	Ok(())
}

/// Undoes the steps in `done`, newest first, and returns the paths that could
/// not be restored. A backup is never removed here: once its file is back in
/// place there is nothing left to remove, and if restoring it failed it holds
/// the only copy.
fn roll_back(done: &mut [Step]) -> Vec<RepoPath> {
	let mut unrestored = Vec::new();
	for step in done.iter_mut().rev() {
		let restored = match step {
			Step::Delete { target, backup, .. } => {
				backup.armed = false;
				fs::rename(&backup.path, target)
			}
			Step::Write {
				target, original, ..
			} => match original {
				Some(original) => Staged::create(
					target,
					&original.contents,
					Some(&original.permissions),
					"tmp",
				)
				.and_then(|mut staged| {
					fs::rename(&staged.path, &*target)?;
					staged.armed = false;
					Ok(())
				}),
				None => fs::remove_file(&*target),
			},
		};
		if restored.is_err() {
			unrestored.push(step.path().clone());
		}
	}
	unrestored
}

/// Removes directories this plan created, innermost first, where they are
/// empty.
fn remove_directories(created: &[PathBuf]) {
	for directory in created.iter().rev() {
		// Best effort: a directory that is not empty holds something else.
		let _ = fs::remove_dir(directory);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A write step over `name`, which holds `old` beforehand when given.
	fn write_step(
		directory: &Path,
		name: &str,
		old: Option<&[u8]>,
		new: &[u8],
	) -> Step {
		let target = directory.join(name);
		let original = old.map(|contents| {
			fs::write(&target, contents).unwrap();
			Original {
				contents: contents.to_vec(),
				permissions: fs::metadata(&target).unwrap().permissions(),
			}
		});
		Step::Write {
			path: RepoPath::new(name).unwrap(),
			staged: Staged::create(&target, new, None, "tmp").unwrap(),
			target,
			original,
		}
	}

	/// A delete step for `name`, which holds `contents`.
	fn delete_step(directory: &Path, name: &str, contents: &[u8]) -> Step {
		let target = directory.join(name);
		fs::write(&target, contents).unwrap();
		Step::Delete {
			path: RepoPath::new(name).unwrap(),
			backup: Staged::create(&target, b"", None, "bak").unwrap(),
			target,
		}
	}

	/// The names in `directory`.
	fn names(directory: &Path) -> Vec<String> {
		let mut names: Vec<String> = fs::read_dir(directory)
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	/// Steps that delete `gone.lock` and `old.lock`, replace `a.md`, and
	/// create `new.md`.
	fn plan_steps(directory: &Path) -> Vec<Step> {
		vec![
			delete_step(directory, "gone.lock", b"gone"),
			delete_step(directory, "old.lock", b"old"),
			write_step(directory, "a.md", Some(b"old a"), b"new a"),
			write_step(directory, "new.md", None, b"new"),
		]
	}

	#[test]
	fn a_failure_at_any_step_restores_everything() {
		for failing in 1..=4 {
			let directory = tempfile::tempdir().unwrap();
			let mut steps = plan_steps(directory.path());
			let mut calls = 0;
			let result = commit(&mut steps, &mut |from, to| {
				calls += 1;
				if calls == failing {
					Err(io::Error::other("injected"))
				} else {
					fs::rename(from, to)
				}
			});
			let (failed, _) = result.unwrap_err();
			assert_eq!(failed, failing - 1);
			assert_eq!(roll_back(&mut steps[..failed]), Vec::<RepoPath>::new());
			drop(steps);

			let read = |name: &str| fs::read(directory.path().join(name)).unwrap();
			assert_eq!(read("gone.lock"), b"gone", "failing step {failing}");
			assert_eq!(read("old.lock"), b"old", "failing step {failing}");
			assert_eq!(read("a.md"), b"old a", "failing step {failing}");
			assert_eq!(
				names(directory.path()),
				["a.md", "gone.lock", "old.lock"],
				"failing step {failing}"
			);
		}
	}

	#[test]
	fn success_removes_the_backups() {
		let directory = tempfile::tempdir().unwrap();
		let mut steps = plan_steps(directory.path());
		commit(&mut steps, &mut |from, to| fs::rename(from, to)).unwrap();
		drop(steps);

		assert_eq!(names(directory.path()), ["a.md", "new.md"]);
		assert_eq!(fs::read(directory.path().join("a.md")).unwrap(), b"new a");
	}

	#[test]
	fn a_backup_that_cannot_be_restored_is_kept() {
		let directory = tempfile::tempdir().unwrap();
		let mut steps = vec![delete_step(directory.path(), "gone.lock", b"gone")];
		commit(&mut steps, &mut |from, to| fs::rename(from, to)).unwrap();
		// Something now occupies the deleted path with a directory, so the
		// backup cannot be renamed back.
		fs::create_dir(directory.path().join("gone.lock")).unwrap();
		fs::write(directory.path().join("gone.lock").join("inner"), b"x").unwrap();

		assert_eq!(roll_back(&mut steps), [RepoPath::new("gone.lock").unwrap()]);
		drop(steps);
		let backups: Vec<String> = names(directory.path())
			.into_iter()
			.filter(|name| name.ends_with(".source-sentry.bak"))
			.collect();
		assert_eq!(backups.len(), 1);
		let backup = directory.path().join(&backups[0]);
		assert_eq!(fs::read(backup).unwrap(), b"gone");
	}
}
