//! Canonical repository paths.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A canonical repository-relative path: `/` separators, no `.` or `..`
/// components, no empty components and no leading or trailing `/`.
///
/// The empty path is the repository root. Two paths are equal exactly when
/// they name the same location, which is what makes double-claim detection
/// work.
#[derive(
	Clone,
	Debug,
	Default,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct RepoPath(String);

/// Why a path was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
	/// The path is empty where a path is required.
	#[error("the path is empty")]
	Empty,
	/// The path uses `\`. Source Sentry paths use `/` only, so a path reads the
	/// same on every machine.
	#[error("`{0}` contains a backslash; use `/` to separate components")]
	Backslash(String),
	/// The path is a filesystem-absolute, drive-letter or UNC path, which an
	/// attestation must not depend on.
	#[error("`{0}` is a filesystem path; use a repository path instead")]
	FilesystemAbsolute(String),
	/// The path climbs above the repository root.
	#[error("`{0}` climbs above the repository root")]
	EscapesRepository(String),
	/// The path contains a control character.
	#[error("`{0:?}` contains a control character")]
	ControlCharacter(String),
	/// A remote document's source is not `/`-rooted.
	#[error("`{0}` is relative; a remote document's sources must start with `/`")]
	NotRooted(String),
}

impl RepoPath {
	/// The repository root.
	pub const ROOT: Self = Self(String::new());

	/// Parses a repository-relative path and normalizes it lexically, so
	/// `a/./b/../c` becomes `a/c`. The empty string and `.` are the root.
	///
	/// # Errors
	///
	/// Rejects backslashes, control characters, leading `/`, drive letters and
	/// paths that climb above the root.
	pub fn new(path: &str) -> Result<Self, PathError> {
		reject_forbidden(path)?;
		if path.starts_with('/') {
			return Err(PathError::FilesystemAbsolute(path.to_owned()));
		}
		normalize(&Self::ROOT, path, path)
	}

	/// The path as a string, without a leading `/`. The root is empty.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}

	/// Whether this is the repository root.
	#[must_use]
	pub fn is_root(&self) -> bool {
		self.0.is_empty()
	}

	/// The directory containing this path, or `None` for the root.
	#[must_use]
	pub fn parent(&self) -> Option<Self> {
		if self.is_root() {
			return None;
		}
		Some(match self.0.rsplit_once('/') {
			Some((parent, _)) => Self(parent.to_owned()),
			None => Self::ROOT,
		})
	}

	/// The last component, or `None` for the root.
	#[must_use]
	pub fn file_name(&self) -> Option<&str> {
		if self.is_root() {
			return None;
		}
		Some(
			self
				.0
				.rsplit_once('/')
				.map_or(self.0.as_str(), |(_, name)| name),
		)
	}

	/// The components from the root down.
	pub fn components(&self) -> impl Iterator<Item = &str> {
		self.0.split('/').filter(|component| !component.is_empty())
	}

	/// Joins a relative path onto this one and normalizes the result.
	///
	/// # Errors
	///
	/// Fails on the same inputs as [`RepoPath::new`].
	pub fn join(&self, relative: &str) -> Result<Self, PathError> {
		reject_forbidden(relative)?;
		if relative.starts_with('/') {
			return Err(PathError::FilesystemAbsolute(relative.to_owned()));
		}
		normalize(self, relative, relative)
	}

	/// Whether `ancestor` is this path or one of its ancestors, compared by
	/// component rather than by text, so `docs` is not an ancestor of
	/// `docs-old/a.md`.
	#[must_use]
	pub fn starts_with(&self, ancestor: &Self) -> bool {
		ancestor.is_root()
			|| self.0 == ancestor.0
			|| self
				.0
				.strip_prefix(ancestor.0.as_str())
				.is_some_and(|rest| rest.starts_with('/'))
	}

	/// This path relative to `ancestor`, or `None` when `ancestor` is not an
	/// ancestor. The result is empty when both are equal.
	#[must_use]
	pub fn strip_prefix(&self, ancestor: &Self) -> Option<&str> {
		if ancestor.is_root() {
			return Some(&self.0);
		}
		if self.0 == ancestor.0 {
			return Some("");
		}
		self
			.0
			.strip_prefix(ancestor.0.as_str())
			.and_then(|rest| rest.strip_prefix('/'))
	}
}

impl fmt::Display for RepoPath {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		if self.is_root() {
			formatter.write_str("/")
		} else {
			formatter.write_str(&self.0)
		}
	}
}

impl TryFrom<String> for RepoPath {
	type Error = PathError;

	fn try_from(path: String) -> Result<Self, Self::Error> {
		Self::new(&path)
	}
}

impl From<RepoPath> for String {
	fn from(path: RepoPath) -> Self {
		path.0
	}
}

/// Resolves a source path as a document declares it.
///
/// `/PATH` is relative to the repository root. A bare `PATH`, `./PATH` and
/// `../PATH` are relative to the directory containing `document`, and may
/// climb no higher than the repository root.
///
/// # Errors
///
/// Rejects empty paths, backslashes, control characters, filesystem-absolute
/// paths (`//server`, `C:/`) and paths that climb above the root.
pub fn resolve_source(
	document: &RepoPath,
	declared: &str,
) -> Result<RepoPath, PathError> {
	reject_forbidden(declared)?;
	if declared.is_empty() {
		return Err(PathError::Empty);
	}
	if let Some(rooted) = declared.strip_prefix('/') {
		if rooted.starts_with('/') {
			return Err(PathError::FilesystemAbsolute(declared.to_owned()));
		}
		return normalize(&RepoPath::ROOT, rooted, declared);
	}
	let directory = document.parent().unwrap_or(RepoPath::ROOT);
	normalize(&directory, declared, declared)
}

/// Resolves a remote document's source, which must be `/`-rooted because a
/// remote document has no directory.
///
/// # Errors
///
/// Fails like [`resolve_source`], and with [`PathError::NotRooted`] when the
/// path does not start with `/`.
pub fn resolve_rooted(declared: &str) -> Result<RepoPath, PathError> {
	if !declared.starts_with('/') {
		return Err(PathError::NotRooted(declared.to_owned()));
	}
	resolve_source(&RepoPath::ROOT, declared)
}

/// Rejects spellings that no Source Sentry path may use.
fn reject_forbidden(path: &str) -> Result<(), PathError> {
	if path.chars().any(char::is_control) {
		return Err(PathError::ControlCharacter(path.to_owned()));
	}
	if path.contains('\\') {
		return Err(PathError::Backslash(path.to_owned()));
	}
	let bytes = path.as_bytes();
	if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
		return Err(PathError::FilesystemAbsolute(path.to_owned()));
	}
	Ok(())
}

/// Applies `relative` to `base` component by component. `original` is the
/// input as written, for error messages.
fn normalize(
	base: &RepoPath,
	relative: &str,
	original: &str,
) -> Result<RepoPath, PathError> {
	let mut components: Vec<&str> = base.components().collect();
	for component in relative.split('/') {
		match component {
			"" | "." => {}
			".." => {
				if components.pop().is_none() {
					return Err(PathError::EscapesRepository(original.to_owned()));
				}
			}
			name => components.push(name),
		}
	}
	Ok(RepoPath(components.join("/")))
}
