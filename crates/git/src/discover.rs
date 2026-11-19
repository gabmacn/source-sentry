//! Finds the repository, working tree and object directory that `git` itself
//! uses for a command started in a directory, with the process's
//! environment: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`,
//! `GIT_OBJECT_DIRECTORY`, `GIT_CEILING_DIRECTORIES`, `safe.directory` and
//! the rest apply exactly as git applies them, because git is asked.

use std::path::{Path, PathBuf};

use source_sentry_core::ports::{BackendError, BackendErrorKind, RepoPath};

use crate::{
	command::{Finished, unbound},
	error::failure,
};

/// What git reports about the repository it would use.
#[derive(Debug)]
pub(crate) struct Location {
	/// The git directory, absolute.
	pub git_dir: PathBuf,
	/// The git directory shared by all worktrees, absolute.
	pub common_dir: PathBuf,
	/// The working tree's root, absolute, or `None` for a bare repository.
	pub work_dir: Option<PathBuf>,
	/// Where the start directory lies in the working tree, or `None` when it
	/// lies outside it or the repository is bare.
	pub prefix: Option<RepoPath>,
}

/// The paths and flags of a repository, in the order git prints them.
/// `--show-toplevel` comes last, because git stops there in a bare
/// repository and inside a git directory.
const QUESTIONS: [&str; 11] = [
	"rev-parse",
	"--path-format=absolute",
	"--git-dir",
	"--git-common-dir",
	"--git-path",
	"objects",
	"--is-bare-repository",
	"--is-inside-work-tree",
	"--is-inside-git-dir",
	"--show-prefix",
	"--show-toplevel",
];

/// The questions that git answers without a working tree, to explain why
/// the full set failed.
const PROBE: [&str; 8] = [
	"rev-parse",
	"--path-format=absolute",
	"--git-dir",
	"--git-common-dir",
	"--git-path",
	"objects",
	"--is-bare-repository",
	"--is-inside-git-dir",
];

/// Asks git which repository a command started in `start` uses.
///
/// # Errors
///
/// Fails with [`BackendErrorKind::Git`] when git finds no repository or
/// refuses it, when `start` lies inside a git directory, when the working
/// tree git reports is not a directory, and when a reported path holds a
/// line break; with [`BackendErrorKind::GitExecutable`] when `git` cannot
/// run.
pub(crate) fn locate(start: &Path) -> Result<Location, BackendError> {
	let finished = unbound(start, &QUESTIONS)?;
	if finished.status.success() {
		let [
			git_dir,
			common_dir,
			_objects,
			_bare,
			inside,
			_in_git_dir,
			prefix,
			top,
		] = answers::<8>(start, &finished)?;
		let work_dir = path(start, top)?;
		if !work_dir.is_dir() {
			return Err(failure(
				BackendErrorKind::Git,
				format!(
					"the working tree {} that git uses does not exist or is not a \
					 directory",
					work_dir.display()
				),
			));
		}
		let prefix = if inside == b"true" {
			Some(prefix_path(start, prefix)?)
		} else {
			None
		};
		return Ok(Location {
			git_dir: path(start, git_dir)?,
			common_dir: path(start, common_dir)?,
			work_dir: Some(work_dir),
			prefix,
		});
	}
	let probe = unbound(start, &PROBE)?;
	if probe.status.success() {
		let [git_dir, common_dir, _objects, bare, in_git_dir] =
			answers::<5>(start, &probe)?;
		let git_dir = path(start, git_dir)?;
		if bare == b"true" {
			return Ok(Location {
				git_dir,
				common_dir: path(start, common_dir)?,
				work_dir: None,
				prefix: None,
			});
		}
		if in_git_dir == b"true" {
			return Err(failure(
				BackendErrorKind::Git,
				format!(
					"{} lies inside the git directory {}; start in the working tree \
					 instead",
					start.display(),
					git_dir.display()
				),
			));
		}
	}
	let message = String::from_utf8_lossy(&finished.stderr);
	let message = message.trim();
	let message = message.strip_prefix("fatal: ").unwrap_or(message);
	Err(failure(
		BackendErrorKind::Git,
		format!(
			"cannot open a git repository at {}: {message}",
			start.display()
		),
	))
}

/// The `N` lines git printed, one answer each.
///
/// # Errors
///
/// Fails when git printed another number of lines, which happens only when
/// a path holds a line break: `git rev-parse` cannot quote one.
fn answers<'a, const N: usize>(
	start: &Path,
	finished: &'a Finished,
) -> Result<[&'a [u8]; N], BackendError> {
	let output = finished
		.stdout
		.strip_suffix(b"\n")
		.unwrap_or(&finished.stdout);
	let lines: Vec<&[u8]> = output
		.split(|&byte| byte == b'\n')
		.map(|line| line.strip_suffix(b"\r").unwrap_or(line))
		.collect();
	lines.try_into().map_err(|_| {
		failure(
			BackendErrorKind::Git,
			format!(
				"cannot read where the repository of {} is: a path of it holds a \
				 line break",
				start.display()
			),
		)
	})
}

/// A path that git printed.
///
/// # Errors
///
/// Fails, outside Unix, for a path that is not UTF-8, which git for those
/// platforms never prints.
fn path(start: &Path, bytes: &[u8]) -> Result<PathBuf, BackendError> {
	#[cfg(unix)]
	{
		use std::os::unix::ffi::OsStrExt as _;
		let _ = start;
		Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
	}
	#[cfg(not(unix))]
	{
		std::str::from_utf8(bytes).map(PathBuf::from).map_err(|_| {
			failure(
				BackendErrorKind::Git,
				format!(
					"cannot read where the repository of {} is: git printed a path \
					 that is not UTF-8",
					start.display()
				),
			)
		})
	}
}

/// The start directory's place in the working tree, from `--show-prefix`,
/// which ends with `/` except at the root.
///
/// # Errors
///
/// Fails when the directory cannot be a repository path.
fn prefix_path(start: &Path, bytes: &[u8]) -> Result<RepoPath, BackendError> {
	let text = std::str::from_utf8(bytes).map_err(|_| {
		failure(
			BackendErrorKind::Git,
			format!("the path of {} is not UTF-8", start.display()),
		)
	})?;
	RepoPath::new(text.strip_suffix('/').unwrap_or(text)).map_err(|error| {
		failure(
			BackendErrorKind::Git,
			format!(
				"{} cannot be read as a repository path: {error}",
				start.display()
			),
		)
	})
}
