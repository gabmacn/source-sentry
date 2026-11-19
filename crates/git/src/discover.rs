//! Finds the repository, working tree and object directory that `git` itself
//! uses for a command started in a directory, with the process's
//! environment: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`,
//! `GIT_OBJECT_DIRECTORY`, `GIT_CEILING_DIRECTORIES`, `safe.directory` and
//! the rest apply exactly as git applies them, because git is asked.

use std::{
	ffi::OsString,
	path::{Path, PathBuf},
};

use source_sentry_core::ports::{BackendError, BackendErrorKind, RepoPath};

use crate::{
	command::{Finished, exit_failure, unbound},
	error::failure,
};

/// What git reports about the repository it would use.
#[derive(Debug)]
pub(crate) struct Location {
	/// The git directory, absolute.
	pub git_dir: PathBuf,
	/// The git directory shared by all worktrees, absolute.
	pub common_dir: PathBuf,
	/// The object directory, absolute.
	pub objects: PathBuf,
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
			objects,
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
			objects: path(start, objects)?,
			work_dir: Some(work_dir),
			prefix,
		});
	}
	let probe = unbound(start, &PROBE)?;
	if probe.status.success() {
		let [git_dir, common_dir, objects, bare, in_git_dir] =
			answers::<5>(start, &probe)?;
		let git_dir = path(start, git_dir)?;
		if bare == b"true" {
			return Ok(Location {
				git_dir,
				common_dir: path(start, common_dir)?,
				objects: path(start, objects)?,
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

/// The object directories that git adds as alternates, absolute, when
/// `GIT_ALTERNATE_OBJECT_DIRECTORIES` is set: git is asked, with
/// `git count-objects -v`, because it takes a relative entry from the
/// directory it works in, the working tree's root or the start directory,
/// and passes over one that does not exist. The list includes those that
/// the object directory's own `info/alternates` names. `None` when the
/// variable is unset.
///
/// # Errors
///
/// Fails when git fails or prints a path that cannot be read.
pub(crate) fn alternates(
	start: &Path,
) -> Result<Option<Vec<PathBuf>>, BackendError> {
	if std::env::var_os("GIT_ALTERNATE_OBJECT_DIRECTORIES").is_none() {
		return Ok(None);
	}
	let args = ["count-objects", "-v"];
	let finished = unbound(start, &args)?;
	if !finished.status.success() {
		return Err(exit_failure(&args, &finished));
	}
	let mut directories = Vec::new();
	for line in finished.stdout.split(|&byte| byte == b'\n') {
		let line = line.strip_suffix(b"\r").unwrap_or(line);
		let Some(entry) = line.strip_prefix(b"alternate: ") else {
			continue;
		};
		// Git quotes a path as in C when it holds unusual characters.
		let entry = match unquote(entry) {
			Some((unquoted, used)) if used == entry.len() => unquoted,
			_ => entry.to_vec(),
		};
		directories.push(path(start, &entry)?);
	}
	Ok(Some(directories))
}

/// Unquotes a C-style quoted string at the start of `text`, as git's
/// `unquote_c_style` does, returning its bytes and how many bytes of `text`
/// it used, or `None` when it is not a valid quoted string.
fn unquote(text: &[u8]) -> Option<(Vec<u8>, usize)> {
	if text.first() != Some(&b'"') {
		return None;
	}
	let mut out = Vec::new();
	let mut position = 1;
	loop {
		let byte = *text.get(position)?;
		position += 1;
		match byte {
			b'"' => return Some((out, position)),
			b'\\' => {
				let escaped = *text.get(position)?;
				position += 1;
				let plain = match escaped {
					b'a' => 0x07,
					b'b' => 0x08,
					b'f' => 0x0c,
					b'n' => b'\n',
					b'r' => b'\r',
					b't' => b'\t',
					b'v' => 0x0b,
					b'\\' | b'"' => escaped,
					b'0'..=b'3' => {
						let second = *text.get(position)?;
						let third = *text.get(position + 1)?;
						if !(b'0'..=b'7').contains(&second)
							|| !(b'0'..=b'7').contains(&third)
						{
							return None;
						}
						position += 2;
						((escaped - b'0') << 6) | ((second - b'0') << 3) | (third - b'0')
					}
					_ => return None,
				};
				out.push(plain);
			}
			_ => out.push(byte),
		}
	}
}

/// `directories` as a value of `GIT_ALTERNATE_OBJECT_DIRECTORIES`, each
/// quoted as in C so that no separator inside a path splits it.
///
/// # Errors
///
/// Fails, outside Unix, for a path that is not UTF-8.
pub(crate) fn join_alternates(
	directories: &[PathBuf],
) -> Result<OsString, BackendError> {
	let separator = if cfg!(windows) { b';' } else { b':' };
	let mut joined = Vec::new();
	for (position, directory) in directories.iter().enumerate() {
		if position > 0 {
			joined.push(separator);
		}
		let bytes =
			os_bytes(directory.clone().into_os_string()).ok_or_else(|| {
				failure(
					BackendErrorKind::Git,
					format!("the object directory {} is not UTF-8", directory.display()),
				)
			})?;
		joined.push(b'"');
		for byte in bytes {
			match byte {
				b'"' | b'\\' => joined.extend([b'\\', byte]),
				0..0x20 | 0x7f => {
					joined.extend(format!("\\{byte:03o}").bytes());
				}
				_ => joined.push(byte),
			}
		}
		joined.push(b'"');
	}
	bytes_os(joined).ok_or_else(|| {
		failure(BackendErrorKind::Git, "an object directory is not UTF-8")
	})
}

/// The bytes of an environment value: as they are on Unix, and as UTF-8
/// elsewhere, `None` when it is not.
fn os_bytes(value: OsString) -> Option<Vec<u8>> {
	#[cfg(unix)]
	{
		use std::os::unix::ffi::OsStringExt as _;
		Some(value.into_vec())
	}
	#[cfg(not(unix))]
	{
		value.into_string().ok().map(String::into_bytes)
	}
}

/// An environment value from its bytes, the inverse of [`os_bytes`].
fn bytes_os(bytes: Vec<u8>) -> Option<OsString> {
	#[cfg(unix)]
	{
		use std::os::unix::ffi::OsStringExt as _;
		Some(OsString::from_vec(bytes))
	}
	#[cfg(not(unix))]
	{
		String::from_utf8(bytes).ok().map(OsString::from)
	}
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

#[cfg(test)]
mod tests {
	use super::*;

	/// Quoted paths are read as git writes them, and anything else is not
	/// a quoted path.
	#[test]
	fn quoted_paths_are_read_as_git_writes_them() {
		assert_eq!(unquote(br#""a b""#), Some((b"a b".to_vec(), 5)));
		assert_eq!(
			unquote(br#""a\101\\\"\t" rest"#),
			Some((b"aA\\\"\t".to_vec(), 13))
		);
		assert_eq!(unquote(b"plain"), None);
		assert_eq!(unquote(br#""unterminated"#), None);
		assert_eq!(unquote(br#""\9""#), None);
	}

	/// Joined directories are each quoted as in C, so that separators and
	/// quotes inside them stay theirs, and read back unchanged.
	#[test]
	fn joined_alternates_quote_each_directory() {
		let directories = [
			PathBuf::from("plain"),
			PathBuf::from("with;semicolon:colon"),
			PathBuf::from(r#"with "quote" and \ backslash"#),
		];
		let joined = os_bytes(join_alternates(&directories).unwrap()).unwrap();
		let separator = if cfg!(windows) { b';' } else { b':' };
		let mut rest = joined.as_slice();
		let mut read = Vec::new();
		while let Some((entry, used)) = unquote(rest) {
			read.push(PathBuf::from(String::from_utf8(entry).unwrap()));
			rest = &rest[used..];
			rest = rest.strip_prefix(&[separator]).unwrap_or(rest);
		}
		assert_eq!(rest, b"");
		assert_eq!(read, directories);
	}
}
