//! What the working tree holds at a path, without following symbolic links.

use std::{
	collections::HashMap,
	fs, io,
	path::{Path, PathBuf},
};

use source_sentry_core::ports::{BackendError, RepoPath};

use crate::error;

/// What the working tree holds at a repository path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Presence {
	/// A regular file.
	File,
	/// A symbolic link, with its target in git's blob representation.
	Symlink(Vec<u8>),
	/// Nothing git would track: no entry, a directory, or a path below a
	/// symbolic link or a file.
	Absent,
}

/// Inspects working-tree paths, remembering which directories are real
/// directories so that many files in one directory cost one check.
pub(crate) struct Probe<'a> {
	work_dir: &'a Path,
	directories: HashMap<RepoPath, bool>,
}

impl<'a> Probe<'a> {
	/// Inspects paths below `work_dir`.
	pub(crate) fn new(work_dir: &'a Path) -> Self {
		Self {
			work_dir,
			directories: HashMap::new(),
		}
	}

	/// What the working tree holds at `path`. Like git, a path below a
	/// symbolic link is absent, because following the link would read
	/// something else.
	///
	/// # Errors
	///
	/// Fails when the file system reports something other than absence.
	pub(crate) fn presence(
		&mut self,
		path: &RepoPath,
	) -> Result<Presence, BackendError> {
		if let Some(parent) = path.parent()
			&& !self.is_directory(&parent)?
		{
			return Ok(Presence::Absent);
		}
		let full = native(self.work_dir, path);
		match fs::symlink_metadata(&full) {
			Ok(metadata) if metadata.file_type().is_symlink() => {
				let target = fs::read_link(&full).map_err(|source| {
					error::io(format!("cannot read the symbolic link {path}"), source)
				})?;
				Ok(Presence::Symlink(link_bytes(target, path)?))
			}
			Ok(metadata) if metadata.is_file() => Ok(Presence::File),
			Ok(_) => Ok(Presence::Absent),
			Err(source) if is_absence(&source) => Ok(Presence::Absent),
			Err(source) => Err(error::io(format!("cannot inspect {path}"), source)),
		}
	}

	/// Whether the working tree holds a real directory at `path`, below real
	/// directories only, as a checked-out or uninitialized submodule does.
	///
	/// # Errors
	///
	/// Fails when the file system reports something other than absence.
	pub(crate) fn holds_directory(
		&mut self,
		path: &RepoPath,
	) -> Result<bool, BackendError> {
		self.is_directory(path)
	}

	/// Whether `directory` and each of its ancestors is a real directory.
	fn is_directory(
		&mut self,
		directory: &RepoPath,
	) -> Result<bool, BackendError> {
		if directory.is_root() {
			return Ok(true);
		}
		if let Some(&known) = self.directories.get(directory) {
			return Ok(known);
		}
		let parent_ok = match directory.parent() {
			Some(parent) => self.is_directory(&parent)?,
			None => true,
		};
		let real = parent_ok
			&& match fs::symlink_metadata(native(self.work_dir, directory)) {
				Ok(metadata) => metadata.is_dir(),
				Err(source) if is_absence(&source) => false,
				Err(source) => {
					return Err(error::io(format!("cannot inspect {directory}"), source));
				}
			};
		self.directories.insert(directory.clone(), real);
		Ok(real)
	}
}

/// The native path of a repository path below `work_dir`.
pub(crate) fn native(work_dir: &Path, path: &RepoPath) -> PathBuf {
	path
		.components()
		.fold(work_dir.to_path_buf(), |full, component| {
			full.join(component)
		})
}

/// Whether an I/O error means that nothing exists at the path.
pub(crate) fn is_absence(error: &io::Error) -> bool {
	matches!(
		error.kind(),
		io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
	)
}

/// A link target as git stores it: the target's bytes.
#[cfg(unix)]
#[expect(
	clippy::unnecessary_wraps,
	reason = "the signature is shared with the Windows version, which can fail"
)]
fn link_bytes(
	target: PathBuf,
	_path: &RepoPath,
) -> Result<Vec<u8>, BackendError> {
	use std::os::unix::ffi::OsStringExt as _;
	Ok(target.into_os_string().into_vec())
}

/// A link target as git stores it. Git for Windows records targets with `/`
/// separators.
#[cfg(not(unix))]
fn link_bytes(
	target: PathBuf,
	path: &RepoPath,
) -> Result<Vec<u8>, BackendError> {
	let text = target.into_os_string().into_string().map_err(|_| {
		error::failure(
			source_sentry_core::ports::BackendErrorKind::Io,
			format!("the symbolic link {path} has a target that is not Unicode"),
		)
	})?;
	Ok(text.replace('\\', "/").into_bytes())
}
