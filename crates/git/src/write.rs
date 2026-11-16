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
//!    each staged file is renamed over its target. Just before each rename,
//!    the target is checked again: unchanged bytes, or a still-free path for
//!    a new file, pass; anything else is verified again as in phase 1. If a
//!    check or a rename fails, the steps already taken are undone in
//!    reverse: backups are renamed back, replaced files are restored from
//!    the contents read during verification, and new files are removed. A
//!    target that no longer holds what its step left there was changed by
//!    another program meanwhile, and is left as it is.
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
	/// Applying the plan failed, and some of the changes already made were
	/// not undone: undoing them failed, or another program changed the file
	/// after the plan did, and it was left as it is.
	///
	/// A written file in `unrestored` may hold its new contents, or be
	/// missing, and a created one may remain marked intent-to-add. A deleted
	/// file in `unrestored` is missing. A path in `changed` holds what another
	/// program saved there after the plan changed it. The contents of a
	/// deleted file in either list remain beside it in a hidden file named
	/// `.<name>.<process>.<n>.source-sentry.bak`, where a long `<name>` is cut
	/// to its first 64 bytes. When marking failed, `path`
	/// is the first created file, and `source` wraps the [`BackendError`].
	#[error("{}", rollback_failure(path, unrestored, changed))]
	RollbackFailed {
		/// The target whose change failed.
		path: RepoPath,
		/// The underlying failure.
		#[source]
		source: io::Error,
		/// The files that could not be restored.
		unrestored: Vec<RepoPath>,
		/// The files that another program changed after the plan had written
		/// or deleted them, which were left as they are rather than restored.
		changed: Vec<RepoPath>,
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

/// The message of an [`ApplyError::RollbackFailed`]: the change that failed,
/// the paths that could not be restored, and those left as another program
/// changed them.
fn rollback_failure(
	path: &RepoPath,
	unrestored: &[RepoPath],
	changed: &[RepoPath],
) -> String {
	let mut parts = vec![format!("cannot change {path}")];
	if !unrestored.is_empty() {
		parts.push(format!("cannot restore {}", list(unrestored)));
	}
	if !changed.is_empty() {
		let (state, pronoun) = if changed.len() == 1 {
			("it is", "it")
		} else {
			("they are", "them")
		};
		parts.push(format!(
			"left {} as {state}, because another program changed {pronoun} \
			 meanwhile",
			list(changed)
		));
	}
	match parts.split_last() {
		Some((last, rest)) if !rest.is_empty() => {
			format!("{}, and {last}", rest.join(", "))
		}
		_ => parts.concat(),
	}
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
	/// Immediately before each rename, the target is checked again against
	/// what verification found, so that a file another program saves while
	/// the plan is being applied fails the plan, which is then undone, rather
	/// than being overwritten. No file is locked, so a short window remains:
	/// a change made between that last check and the rename itself is still
	/// replaced.
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
	/// changed: a change failed, and an earlier one was not undone, because
	/// undoing it failed or because another program changed that file after
	/// the plan did. Such a file is left as that program saved it.
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
			// Under `core.symlinks=false` a link is a plain-file placeholder,
			// which the filesystem checks below cannot recognize.
			if index.get(path).is_some_and(IndexRecord::is_symlink) {
				return Err(unwritable(path, Unwritable::SymbolicLink));
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
		#[cfg(test)]
		tests::after_verification();

		let mut created_directories = Vec::new();
		let mut steps = match stage(writes, deletes, &mut created_directories) {
			Ok(steps) => steps,
			Err(failure) => {
				remove_directories(&created_directories);
				return Err(failure.into_error());
			}
		};
		let result = commit(
			&mut steps,
			&mut |step| self.recheck(work_dir, &index, plan, step),
			&mut |from, to| fs::rename(from, to),
		);
		let Err((failed, interruption)) = result else {
			if created.is_empty() {
				// Dropping the steps removes the backups of deleted files.
				return Ok(());
			}
			return self.mark_created(&created, steps, &created_directories);
		};
		let path = steps[failed].path().clone();
		let deleting = matches!(steps[failed], Step::Delete { .. });
		let left = roll_back(&mut steps[..failed]);
		drop(steps);
		remove_directories(&created_directories);
		if !left.is_empty() {
			let source = match interruption {
				Interruption::Failed(source) => source,
				Interruption::Changed(error) => io::Error::other(error),
			};
			return Err(ApplyError::RollbackFailed {
				path,
				source,
				unrestored: left.unrestored,
				changed: left.changed,
			});
		}
		Err(match interruption {
			Interruption::Failed(source) => Failure {
				path,
				deleting,
				source,
			}
			.into_error(),
			Interruption::Changed(error) => error,
		})
	}

	/// Checks, just before a step is taken, that its target is still as
	/// verification found it. The directories above it are inspected again,
	/// and one that has become a symbolic link or a file fails the step with
	/// verification's error. Then a file whose bytes, and on Unix
	/// permissions, are unchanged, or a new file's path that is still free,
	/// passes at once. Anything else is verified again exactly as the
	/// verification phase verifies it, and fails with that phase's error. A
	/// rewritten file that still meets the expectation passes, and its
	/// current contents and permissions become what a rollback restores; the
	/// staged replacement takes those permissions too.
	fn recheck(
		&self,
		work_dir: &Path,
		index: &BTreeMap<RepoPath, IndexRecord>,
		plan: &WritePlan,
		step: &mut Step,
	) -> Result<(), ApplyError> {
		match step {
			Step::Write {
				path,
				target,
				staged,
				original,
				..
			} => {
				inspect_parents(work_dir, path)?;
				let contents = original.as_ref().map(|kept| kept.contents.as_slice());
				if unchanged(target, contents)
					&& same_permissions(target, original.as_ref())
				{
					return Ok(());
				}
				let write = plan
					.writes
					.iter()
					.find(|write| write.path == *path)
					.ok_or_else(|| ApplyError::ChangedDuringVerification(path.clone()))?;
				let targets = [WriteTarget {
					write,
					inspected: inspect(work_dir, path)?,
				}];
				self.verify(index, &targets, &[])?;
				let [WriteTarget { inspected, .. }] = targets;
				if let Some(current) = &inspected.original {
					staged
						.set_permissions(&current.permissions)
						.map_err(|source| {
							ApplyError::from(error::io(
								format!("cannot write {path}"),
								source,
							))
						})?;
				}
				*original = inspected.original;
			}
			Step::Delete {
				path,
				target,
				contents,
				..
			} => {
				inspect_parents(work_dir, path)?;
				if unchanged(target, Some(contents)) {
					return Ok(());
				}
				let delete = plan
					.deletes
					.iter()
					.find(|delete| delete.path == *path)
					.ok_or_else(|| ApplyError::ChangedDuringVerification(path.clone()))?;
				let targets = [inspect_delete(work_dir, delete)?];
				self.verify(index, &[], &targets)?;
				let [DeleteTarget { original, .. }] = targets;
				*contents = original.contents;
			}
		}
		Ok(())
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
		let left = roll_back(&mut steps);
		for path in left.unrestored {
			if !unrestored.contains(&path) {
				unrestored.push(path);
			}
		}
		drop(steps);
		remove_directories(created_directories);
		if unrestored.is_empty() && left.changed.is_empty() {
			return Err(ApplyError::IntentToAddFailed {
				paths: created.to_vec(),
				source: error,
			});
		}
		Err(ApplyError::RollbackFailed {
			path: created.first().cloned().unwrap_or_default(),
			source: io::Error::other(error),
			unrestored,
			changed: left.changed,
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
	let missing_directories = inspect_parents(work_dir, path)?;
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

/// Inspects the directories above a target without following symbolic
/// links, and returns those that do not exist yet, outermost first. A
/// directory above the target that is a symbolic link or a file refuses it.
fn inspect_parents(
	work_dir: &Path,
	path: &RepoPath,
) -> Result<Vec<PathBuf>, ApplyError> {
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
	Ok(missing_directories)
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
	/// Unix, and flushes it to disk. `<name>` is the target's name, cut to
	/// [`STAGED_NAME_BYTES`] so that the temporary name stays well within the
	/// file system's limit on one name however long the target's is; `<n>`
	/// tells apart names that are still the same.
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
			.to_string_lossy();
		let name = shortened(&name, STAGED_NAME_BYTES);
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
		if let Some(permissions) = permissions {
			staged.set_permissions(permissions)?;
		}
		Ok(staged)
	}

	/// Gives the file `permissions` on Unix. Only Unix permissions, such as an
	/// executable bit, are copied: a read-only copy on Windows could not be
	/// cleaned up.
	fn set_permissions(&self, permissions: &fs::Permissions) -> io::Result<()> {
		if cfg!(unix) {
			fs::set_permissions(&self.path, permissions.clone())
		} else {
			Ok(())
		}
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

/// The most bytes of a target's name that a temporary name beside it repeats.
/// The rest of the temporary name takes at most 35 bytes, so the whole stays
/// far below the 255 bytes, or UTF-16 units, that common file systems allow
/// in one name.
const STAGED_NAME_BYTES: usize = 64;

/// `name` cut to at most `limit` bytes, on a character boundary.
fn shortened(name: &str, limit: usize) -> &str {
	name.get(..name.floor_char_boundary(limit)).unwrap_or(name)
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
		/// The contents verification found, which the target must still hold.
		contents: Vec<u8>,
	},
	/// Moves a staged content over its target.
	Write {
		path: RepoPath,
		target: PathBuf,
		staged: Staged,
		/// The new contents, which the target holds once the step is taken.
		contents: Vec<u8>,
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

/// Why the commit phase stopped before a step.
#[derive(Debug)]
enum Interruption {
	/// The step's target is no longer as verification found it.
	Changed(ApplyError),
	/// The step's rename failed.
	Failed(io::Error),
}

/// Whether the file at `target` still holds exactly `contents`, or, for
/// `None`, whether nothing is there yet. A symbolic link never matches.
fn unchanged(target: &Path, contents: Option<&[u8]>) -> bool {
	match (fs::symlink_metadata(target), contents) {
		(Ok(metadata), Some(contents)) => {
			metadata.is_file()
				&& fs::read(target).is_ok_and(|current| current == contents)
		}
		(Err(source), None) => is_absence(&source),
		_ => false,
	}
}

/// Whether the file at `target` still has the Unix permissions of
/// `original`, which its replacement was staged with. Elsewhere, and for a
/// new file, there are none to compare.
fn same_permissions(target: &Path, original: Option<&Original>) -> bool {
	#[cfg(unix)]
	if let Some(original) = original {
		use std::os::unix::fs::PermissionsExt as _;
		return fs::symlink_metadata(target).is_ok_and(|metadata| {
			metadata.permissions().mode() & 0o7777
				== original.permissions.mode() & 0o7777
		});
	}
	#[cfg(not(unix))]
	let _ = (target, original);
	true
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
			contents: target.original.contents,
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
			contents: target.write.contents.clone(),
			original: target.inspected.original,
		});
	}
	Ok(steps)
}

/// Takes every step in order: moves each file to delete onto its backup, and
/// each staged content over its target, each only once `check` has found its
/// target as verified. On failure, returns the index of the step that failed;
/// those before it have been taken.
fn commit(
	steps: &mut [Step],
	check: &mut dyn FnMut(&mut Step) -> Result<(), ApplyError>,
	rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), (usize, Interruption)> {
	for (index, step) in steps.iter_mut().enumerate() {
		check(step).map_err(|error| (index, Interruption::Changed(error)))?;
		let failed = |source| (index, Interruption::Failed(source));
		match step {
			Step::Delete { target, backup, .. } => {
				rename(target, &backup.path).map_err(failed)?;
			}
			Step::Write { target, staged, .. } => {
				rename(&staged.path, target).map_err(failed)?;
				staged.armed = false;
			}
		}
		#[cfg(test)]
		if index == 0 {
			tests::after_first_step();
		}
	}
	Ok(())
}

/// The paths that undoing the steps taken did not restore.
#[derive(Debug, Default, PartialEq, Eq)]
struct Left {
	/// The paths whose restoring failed.
	unrestored: Vec<RepoPath>,
	/// The paths that another program changed after their step was taken,
	/// left as they are.
	changed: Vec<RepoPath>,
}

impl Left {
	/// Whether every step was undone.
	fn is_empty(&self) -> bool {
		self.unrestored.is_empty() && self.changed.is_empty()
	}
}

/// Undoes the steps in `done`, newest first, and returns the paths that were
/// not restored. A target that no longer holds what its step left there, the
/// written contents or, for a delete, no file, was changed by another program
/// meanwhile: it is left as it is, so that this newer change is not lost. A
/// backup is never removed here: once its file is back in place there is
/// nothing left to remove, and otherwise it holds the only copy.
fn roll_back(done: &mut [Step]) -> Left {
	let mut left = Left::default();
	for step in done.iter_mut().rev() {
		let restored = match step {
			Step::Delete { target, backup, .. } => {
				backup.armed = false;
				if !unchanged(target, None) {
					left.changed.push(step.path().clone());
					continue;
				}
				fs::rename(&backup.path, target)
			}
			Step::Write {
				target, contents, ..
			} if !unchanged(target, Some(contents)) => {
				left.changed.push(step.path().clone());
				continue;
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
			left.unrestored.push(step.path().clone());
		}
	}
	left
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
	use std::{cell::RefCell, process::Command};

	use source_sentry_core::ports::Snapshot as _;
	use tempfile::TempDir;

	use super::*;

	thread_local! {
		/// Runs once, right after the next plan on this thread is verified, so
		/// that a test can change a target before the plan is committed.
		static AFTER_VERIFICATION: RefCell<Option<Box<dyn FnOnce()>>> =
			const { RefCell::new(None) };
	}

	/// Runs the hook set for this point, if any.
	pub(super) fn after_verification() {
		let hook = AFTER_VERIFICATION.with(|slot| slot.borrow_mut().take());
		if let Some(hook) = hook {
			hook();
		}
	}

	thread_local! {
		/// Runs once, right after the first step of the next commit on this
		/// thread is taken, so that a test can act between two steps.
		static AFTER_FIRST_STEP: RefCell<Option<Box<dyn FnOnce()>>> =
			const { RefCell::new(None) };
	}

	/// Runs the hook set for this point, if any.
	pub(super) fn after_first_step() {
		let hook = AFTER_FIRST_STEP.with(|slot| slot.borrow_mut().take());
		if let Some(hook) = hook {
			hook();
		}
	}

	/// A repository in a temporary directory with `files` committed, and the
	/// working tree's path. Git runs with an empty global configuration.
	fn repository(files: &[(&str, &[u8])]) -> (TempDir, PathBuf) {
		let root = tempfile::tempdir().unwrap();
		fs::write(root.path().join("global.gitconfig"), "").unwrap();
		let work = root.path().join("repo");
		fs::create_dir(&work).unwrap();
		let git = |args: &[&str]| {
			let output = Command::new("git")
				.args(args)
				.current_dir(&work)
				.env("GIT_CONFIG_GLOBAL", root.path().join("global.gitconfig"))
				.env("GIT_CONFIG_NOSYSTEM", "1")
				.env_remove("GIT_DIR")
				.env_remove("GIT_WORK_TREE")
				.env_remove("GIT_INDEX_FILE")
				.output()
				.unwrap();
			assert!(
				output.status.success(),
				"git {args:?}: {}",
				String::from_utf8_lossy(&output.stderr)
			);
		};
		git(&["init", "--quiet"]);
		for (key, value) in [
			("user.name", "Test Author"),
			("user.email", "author@example.com"),
			("commit.gpgsign", "false"),
			("core.autocrlf", "false"),
			("core.safecrlf", "false"),
		] {
			git(&["config", key, value]);
		}
		for (name, contents) in files {
			let file = work.join(name);
			fs::create_dir_all(file.parent().unwrap()).unwrap();
			fs::write(file, contents).unwrap();
		}
		git(&["add", "--all"]);
		git(&["commit", "--quiet", "--message", "files"]);
		(root, work)
	}

	/// The canonical oid of a working-tree file.
	fn oid(work: &Path, name: &str) -> ObjectId {
		let repository = GitRepository::open(work).unwrap();
		let snapshot = repository.working_tree_snapshot().unwrap();
		match snapshot.canonical_oids(&[RepoPath::new(name).unwrap()]) {
			Ok(mut oids) => match oids.pop() {
				Some(Canonical::Oid(id)) => id,
				other => panic!("no oid for {name}: {other:?}"),
			},
			Err(error) => panic!("no oid for {name}: {error}"),
		}
	}

	/// Applies `plan` to the repository at `work`, running `meanwhile` once
	/// the plan has been verified and before anything is changed.
	fn apply_meanwhile(
		work: &Path,
		plan: &WritePlan,
		meanwhile: impl FnOnce() + 'static,
	) -> Result<(), ApplyError> {
		AFTER_VERIFICATION
			.with(|slot| *slot.borrow_mut() = Some(Box::new(meanwhile)));
		let result = GitRepository::open(work).unwrap().apply(plan);
		AFTER_VERIFICATION.with(|slot| slot.borrow_mut().take());
		result
	}

	/// A planned rewrite of `name`, which must hold what it holds now.
	fn rewrite(work: &Path, name: &str, contents: &[u8]) -> PlannedWrite {
		PlannedWrite {
			path: RepoPath::new(name).unwrap(),
			expected: Some(oid(work, name)),
			contents: contents.to_vec(),
		}
	}

	/// Applies a plan that writes `first`, then rewrites `b.md`, whose
	/// rename fails. Between the two steps, `meanwhile` runs with the path of
	/// `first`, and the staged content of `b.md` is removed.
	fn apply_failing_second_step(
		work: &Path,
		first: PlannedWrite,
		meanwhile: impl FnOnce(&Path) + 'static,
	) -> Result<(), ApplyError> {
		let target = work.join(first.path.to_string());
		let plan = WritePlan {
			writes: vec![first, rewrite(work, "b.md", b"planned b\n")],
			deletes: Vec::new(),
		};
		let directory = work.to_path_buf();
		apply_after_first_step(work, &plan, move || {
			meanwhile(&target);
			let staged = names(&directory)
				.into_iter()
				.find(|name| {
					name.starts_with(".b.md.") && name.ends_with(".source-sentry.tmp")
				})
				.unwrap();
			fs::remove_file(directory.join(staged)).unwrap();
		})
	}

	/// Applies `plan` to the repository at `work`, running `meanwhile` once
	/// its first step has been taken.
	fn apply_after_first_step(
		work: &Path,
		plan: &WritePlan,
		meanwhile: impl FnOnce() + 'static,
	) -> Result<(), ApplyError> {
		AFTER_FIRST_STEP
			.with(|slot| *slot.borrow_mut() = Some(Box::new(meanwhile)));
		let result = GitRepository::open(work).unwrap().apply(plan);
		AFTER_FIRST_STEP.with(|slot| slot.borrow_mut().take());
		result
	}

	/// A planned new file `new.md`.
	fn creation() -> PlannedWrite {
		PlannedWrite {
			path: RepoPath::new("new.md").unwrap(),
			expected: None,
			contents: b"planned a\n".to_vec(),
		}
	}

	#[test]
	fn a_save_after_a_step_is_kept_when_a_later_step_fails() {
		for creates in [false, true] {
			let (_root, work) =
				repository(&[("a.md", b"original a\n"), ("b.md", b"original b\n")]);
			let first = if creates {
				creation()
			} else {
				rewrite(&work, "a.md", b"planned a\n")
			};
			let name = first.path.clone();
			let result = apply_failing_second_step(&work, first, |target| {
				assert_eq!(fs::read(target).unwrap(), b"planned a\n");
				fs::write(target, b"saved by an editor\n").unwrap();
			});

			let error = result.unwrap_err();
			match &error {
				ApplyError::RollbackFailed {
					path,
					unrestored,
					changed,
					..
				} => {
					assert_eq!(*path, RepoPath::new("b.md").unwrap());
					assert_eq!(*unrestored, Vec::<RepoPath>::new());
					assert_eq!(changed.as_slice(), std::slice::from_ref(&name));
				}
				other => panic!("expected a path left changed, got {other:?}"),
			}
			assert_eq!(
				error.to_string(),
				format!(
					"cannot change b.md, and left {name} as it is, because \
					 another program changed it meanwhile"
				)
			);
			assert_eq!(
				fs::read(work.join(name.to_string())).unwrap(),
				b"saved by an editor\n"
			);
			assert_eq!(fs::read(work.join("b.md")).unwrap(), b"original b\n");
			if creates {
				assert_eq!(fs::read(work.join("a.md")).unwrap(), b"original a\n");
			}
		}
	}

	#[test]
	fn a_step_left_as_it_was_is_undone_when_a_later_step_fails() {
		for creates in [false, true] {
			let (_root, work) =
				repository(&[("a.md", b"original a\n"), ("b.md", b"original b\n")]);
			let first = if creates {
				creation()
			} else {
				rewrite(&work, "a.md", b"planned a\n")
			};
			let result = apply_failing_second_step(&work, first, |_| {});

			match result {
				Err(ApplyError::WriteFailed { path, .. }) => {
					assert_eq!(path, RepoPath::new("b.md").unwrap());
				}
				other => panic!("expected a failed write, got {other:?}"),
			}
			assert_eq!(fs::read(work.join("a.md")).unwrap(), b"original a\n");
			assert_eq!(fs::read(work.join("b.md")).unwrap(), b"original b\n");
			assert_eq!(names(&work), [".git", "a.md", "b.md"]);
		}
	}

	#[test]
	fn a_file_saved_after_verification_is_not_overwritten() {
		let (_root, work) =
			repository(&[("a.md", b"old a\n"), ("b.md", b"old b\n")]);
		let plan = WritePlan {
			writes: vec![
				rewrite(&work, "a.md", b"new a\n"),
				rewrite(&work, "b.md", b"new b\n"),
			],
			deletes: Vec::new(),
		};
		let edited = work.join("b.md");
		let result = apply_meanwhile(&work, &plan, move || {
			fs::write(edited, b"saved by an editor\n").unwrap();
		});

		match result {
			Err(ApplyError::Stale {
				path,
				expected,
				actual,
			}) => {
				assert_eq!(path, RepoPath::new("b.md").unwrap());
				assert_eq!(expected, plan.writes[1].expected);
				assert_eq!(actual, Some(oid(&work, "b.md")));
			}
			other => panic!("expected a stale plan, got {other:?}"),
		}
		// The rewrite already made is undone, and the saved file is kept.
		assert_eq!(fs::read(work.join("a.md")).unwrap(), b"old a\n");
		assert_eq!(
			fs::read(work.join("b.md")).unwrap(),
			b"saved by an editor\n"
		);
		assert_eq!(names(&work), [".git", "a.md", "b.md"]);
	}

	#[test]
	fn a_new_file_saved_after_verification_is_not_overwritten() {
		let (_root, work) = repository(&[("a.md", b"a\n")]);
		let plan = WritePlan {
			writes: vec![PlannedWrite {
				path: RepoPath::new("new.md").unwrap(),
				expected: None,
				contents: b"planned\n".to_vec(),
			}],
			deletes: Vec::new(),
		};
		let created = work.join("new.md");
		let result = apply_meanwhile(&work, &plan, move || {
			fs::write(created, b"saved by an editor\n").unwrap();
		});

		assert!(
			matches!(
				&result,
				Err(ApplyError::Stale {
					expected: None,
					actual: Some(_),
					..
				})
			),
			"{result:?}"
		);
		assert_eq!(
			fs::read(work.join("new.md")).unwrap(),
			b"saved by an editor\n"
		);
		assert_eq!(names(&work), [".git", "a.md", "new.md"]);
	}

	#[test]
	fn a_file_to_delete_saved_after_verification_is_kept() {
		let (_root, work) = repository(&[("gone.lock", b"{}\n")]);
		let plan = WritePlan {
			writes: Vec::new(),
			deletes: vec![PlannedDelete {
				path: RepoPath::new("gone.lock").unwrap(),
				expected: oid(&work, "gone.lock"),
			}],
		};
		let edited = work.join("gone.lock");
		let result = apply_meanwhile(&work, &plan, move || {
			fs::write(edited, b"{\"saved\": true}\n").unwrap();
		});

		assert!(
			matches!(
				&result,
				Err(ApplyError::Stale {
					actual: Some(_),
					..
				})
			),
			"{result:?}"
		);
		assert_eq!(
			fs::read(work.join("gone.lock")).unwrap(),
			b"{\"saved\": true}\n"
		);
		assert_eq!(names(&work), [".git", "gone.lock"]);
	}

	/// Makes `link` a symbolic link to the directory `target`, or returns
	/// `false` where this process may not create one.
	#[cfg(any(unix, windows))]
	fn link_directory(target: &Path, link: &Path) -> bool {
		#[cfg(unix)]
		let made = std::os::unix::fs::symlink(target, link);
		#[cfg(windows)]
		let made = std::os::windows::fs::symlink_dir(target, link);
		match made {
			Ok(()) => true,
			// ERROR_PRIVILEGE_NOT_HELD: neither administrator rights nor
			// Developer Mode.
			#[cfg(windows)]
			Err(error) if error.raw_os_error() == Some(1314) => false,
			Err(error) => panic!("cannot link {}: {error}", link.display()),
		}
	}

	#[cfg(any(unix, windows))]
	#[test]
	fn a_parent_directory_linked_after_verification_is_refused() {
		let (root, work) = repository(&[("docs/a.md", b"a\n")]);
		let probe = root.path().join("probe");
		if !link_directory(&work, &probe) {
			return;
		}
		fs::remove_dir(&probe)
			.or_else(|_| fs::remove_file(&probe))
			.unwrap();
		let plan = WritePlan {
			writes: vec![rewrite(&work, "docs/a.md", b"new a\n")],
			deletes: Vec::new(),
		};
		// The directory moves away, and a link to it takes its place, so the
		// target reached through the link holds the same bytes.
		let docs = work.join("docs");
		let moved = root.path().join("moved");
		let linked = moved.clone();
		let result = apply_meanwhile(&work, &plan, move || {
			fs::rename(&docs, &linked).unwrap();
			assert!(link_directory(&linked, &docs));
		});

		match result {
			Err(ApplyError::Unwritable { path, reason }) => {
				assert_eq!(path, RepoPath::new("docs/a.md").unwrap());
				assert_eq!(reason, Unwritable::SymbolicLink);
			}
			other => panic!("expected a refused link, got {other:?}"),
		}
		assert_eq!(fs::read(moved.join("a.md")).unwrap(), b"a\n");
		assert_eq!(names(&moved), ["a.md"]);
	}

	#[cfg(unix)]
	#[test]
	fn a_permission_change_after_staging_is_kept() {
		use std::os::unix::fs::PermissionsExt as _;

		let (_root, work) = repository(&[("a.md", b"a\n"), ("b.md", b"b\n")]);
		let plan = WritePlan {
			writes: vec![
				rewrite(&work, "b.md", b"new b\n"),
				rewrite(&work, "a.md", b"new a\n"),
			],
			deletes: Vec::new(),
		};
		let target = work.join("a.md");
		let mode =
			|path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
		let executable = mode(&target) | 0o111;
		assert_ne!(mode(&target), executable);
		// Both replacements are staged by now, `a.md`'s with the mode read
		// before the change.
		let changed = target.clone();
		apply_after_first_step(&work, &plan, move || {
			fs::set_permissions(&changed, fs::Permissions::from_mode(executable))
				.unwrap();
		})
		.unwrap();

		assert_eq!(fs::read(&target).unwrap(), b"new a\n");
		assert_eq!(mode(&target), executable);
	}

	/// A save that changes the bytes but not the canonical oid still meets
	/// the plan's expectation, as it would have before verification.
	#[test]
	fn a_save_that_keeps_the_canonical_oid_is_replaced() {
		let (_root, work) =
			repository(&[(".gitattributes", b"*.md text\n"), ("a.md", b"a\n")]);
		let plan = WritePlan {
			writes: vec![rewrite(&work, "a.md", b"new a\n")],
			deletes: Vec::new(),
		};
		let edited = work.join("a.md");
		apply_meanwhile(&work, &plan, move || {
			fs::write(edited, b"a\r\n").unwrap();
		})
		.unwrap();

		assert_eq!(fs::read(work.join("a.md")).unwrap(), b"new a\n");
	}

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
			contents: new.to_vec(),
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
			contents: contents.to_vec(),
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
			let result = commit(&mut steps, &mut |_| Ok(()), &mut |from, to| {
				calls += 1;
				if calls == failing {
					Err(io::Error::other("injected"))
				} else {
					fs::rename(from, to)
				}
			});
			let (failed, _) = result.unwrap_err();
			assert_eq!(failed, failing - 1);
			assert_eq!(roll_back(&mut steps[..failed]), Left::default());
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
		commit(&mut steps, &mut |_| Ok(()), &mut |from, to| {
			fs::rename(from, to)
		})
		.unwrap();
		drop(steps);

		assert_eq!(names(directory.path()), ["a.md", "new.md"]);
		assert_eq!(fs::read(directory.path().join("a.md")).unwrap(), b"new a");
	}

	#[test]
	fn temporary_names_repeat_at_most_a_bounded_part_of_the_target_name() {
		assert_eq!(shortened("short.md", 64), "short.md");
		let long = "é".repeat(100);
		let cut = shortened(&long, 64);
		assert_eq!(cut, "é".repeat(32));
		assert_eq!(
			shortened(&format!("a{long}"), 64),
			format!("a{}", "é".repeat(31))
		);

		let directory = tempfile::tempdir().unwrap();
		let target = directory.path().join(format!("{}.md", "x".repeat(247)));
		let staged = Staged::create(&target, b"new", None, "tmp").unwrap();
		let name = staged.path.file_name().unwrap().len();
		assert!(name <= 1 + STAGED_NAME_BYTES + 35, "{name}");
	}

	#[test]
	fn steps_whose_targets_changed_since_are_left_as_they_are() {
		let directory = tempfile::tempdir().unwrap();
		let mut steps = plan_steps(directory.path());
		commit(&mut steps, &mut |_| Ok(()), &mut |from, to| {
			fs::rename(from, to)
		})
		.unwrap();
		let write = |name: &str, contents: &[u8]| {
			fs::write(directory.path().join(name), contents).unwrap();
		};
		write("gone.lock", b"saved again");
		write("a.md", b"saved a");
		write("new.md", b"saved new");

		assert_eq!(
			roll_back(&mut steps),
			Left {
				unrestored: Vec::new(),
				changed: ["new.md", "a.md", "gone.lock"]
					.map(|name| RepoPath::new(name).unwrap())
					.into(),
			}
		);
		drop(steps);
		let read = |name: &str| fs::read(directory.path().join(name)).unwrap();
		assert_eq!(read("gone.lock"), b"saved again");
		assert_eq!(read("old.lock"), b"old");
		assert_eq!(read("a.md"), b"saved a");
		assert_eq!(read("new.md"), b"saved new");
		// The backup of the deleted file that was saved again is kept.
		let backups: Vec<String> = names(directory.path())
			.into_iter()
			.filter(|name| name.ends_with(".source-sentry.bak"))
			.collect();
		assert_eq!(backups.len(), 1);
		assert_eq!(
			fs::read(directory.path().join(&backups[0])).unwrap(),
			b"gone"
		);
	}

	#[test]
	fn rollback_failures_name_both_kinds_of_path() {
		let path = |name: &str| RepoPath::new(name).unwrap();
		let message = |unrestored: &[&str], changed: &[&str]| {
			ApplyError::RollbackFailed {
				path: path("b.md"),
				source: io::Error::other("failed"),
				unrestored: unrestored.iter().map(|name| path(name)).collect(),
				changed: changed.iter().map(|name| path(name)).collect(),
			}
			.to_string()
		};
		assert_eq!(
			message(&["a.md"], &[]),
			"cannot change b.md, and cannot restore a.md"
		);
		assert_eq!(
			message(&[], &["a.md", "c.md"]),
			"cannot change b.md, and left a.md, c.md as they are, because \
			 another program changed them meanwhile"
		);
		assert_eq!(
			message(&["a.md"], &["c.md"]),
			"cannot change b.md, cannot restore a.md, and left c.md as it is, \
			 because another program changed it meanwhile"
		);
	}

	#[test]
	fn a_restore_that_fails_is_reported() {
		let directory = tempfile::tempdir().unwrap();
		let mut steps = vec![delete_step(directory.path(), "gone.lock", b"gone")];
		commit(&mut steps, &mut |_| Ok(()), &mut |from, to| {
			fs::rename(from, to)
		})
		.unwrap();
		// The backup disappears, so the deleted file cannot be renamed back.
		let Step::Delete { backup, .. } = &steps[0] else {
			panic!("expected a delete step");
		};
		fs::remove_file(&backup.path).unwrap();

		assert_eq!(
			roll_back(&mut steps),
			Left {
				unrestored: vec![RepoPath::new("gone.lock").unwrap()],
				changed: Vec::new(),
			}
		);
	}

	#[test]
	fn a_backup_whose_path_is_taken_meanwhile_is_kept() {
		let directory = tempfile::tempdir().unwrap();
		let mut steps = vec![delete_step(directory.path(), "gone.lock", b"gone")];
		commit(&mut steps, &mut |_| Ok(()), &mut |from, to| {
			fs::rename(from, to)
		})
		.unwrap();
		// Something now occupies the deleted path with a directory, which is
		// left as it is.
		fs::create_dir(directory.path().join("gone.lock")).unwrap();
		fs::write(directory.path().join("gone.lock").join("inner"), b"x").unwrap();

		assert_eq!(
			roll_back(&mut steps),
			Left {
				unrestored: Vec::new(),
				changed: vec![RepoPath::new("gone.lock").unwrap()],
			}
		);
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
