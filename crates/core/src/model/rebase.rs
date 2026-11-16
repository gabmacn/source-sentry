//! Re-expressing source declarations between directories.

use globset::GlobBuilder;

use super::{PathError, RepoPath};

/// Why a source cannot be re-expressed safely.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RebaseError {
	/// A literal path is invalid or leaves the repository.
	#[error(transparent)]
	Path(#[from] PathError),
	/// A parent component follows a glob component.
	#[error("`..` follows a wildcard; write the directories out instead")]
	ParentAfterWildcard,
	/// A directory would introduce glob syntax into the resolved pattern.
	#[error("the directory `{0}` contains glob metacharacters")]
	PatternDirectory(RepoPath),
	/// The glob syntax is invalid.
	#[error("invalid glob syntax: {0}")]
	Glob(String),
}

/// Re-expresses `declared`, a source path written relative to the directory `from`, relative to the directory `to`. A `/`-rooted spelling is returned unchanged.
///
/// # Errors
///
/// Rejects empty relative paths, invalid path characters and paths that
/// climb above the repository root.
pub fn rebase_path(
	declared: &str,
	from: &RepoPath,
	to: &RepoPath,
) -> Result<String, RebaseError> {
	if declared.starts_with('/') || declared.starts_with(":/") {
		return Ok(declared.to_owned());
	}
	if declared.is_empty() {
		return Err(PathError::Empty.into());
	}
	Ok(relative(&from.join(declared)?, to))
}

/// The same for a source pattern, rebased by its literal prefix.
///
/// A `/`-rooted spelling is returned unchanged. The suffix beginning with
/// the first component containing `*`, `?`, `[`, `]`, `{` or `}` is kept.
/// The document's own directory is taken literally whatever its name, so
/// `from` and `to` may contain those characters as long as the new spelling
/// does not have to name such a directory: a pattern kept in the same
/// directory, or one that only climbs out of `to` with `..`, is safe.
///
/// # Errors
///
/// Rejects invalid paths or glob syntax, `..` after a glob component, and a
/// new spelling that would name a directory of `from` containing glob
/// metacharacters, which would be read as glob syntax.
pub fn rebase_pattern(
	declared: &str,
	from: &RepoPath,
	to: &RepoPath,
) -> Result<String, RebaseError> {
	if declared.starts_with('/') || declared.starts_with(":/") {
		return Ok(declared.to_owned());
	}
	// Validate characters before interpreting the glob components.
	from.join(declared)?;
	let Some(first) = declared.find(['*', '?', '[', ']', '{', '}']) else {
		return literal(rebase_path(declared, from, to)?, from);
	};
	let split = declared[..first].rfind('/').map_or(0, |index| index + 1);
	let (prefix, suffix) = declared.split_at(split);
	let mut components: Vec<&str> = Vec::new();
	for component in suffix.split('/') {
		match component {
			"" | "." => {}
			".." => {
				if components
					.last()
					.is_none_or(|component| has_glob(component))
				{
					return Err(RebaseError::ParentAfterWildcard);
				}
				components.pop();
			}
			component => components.push(component),
		}
	}
	GlobBuilder::new(declared)
		.literal_separator(true)
		.backslash_escape(false)
		.build()
		.map_err(|error| RebaseError::Glob(error.kind().to_string()))?;
	let prefix = literal(relative(&from.join(prefix)?, to), from)?;
	Ok(if prefix == "." {
		protect_relative(suffix)
	} else {
		format!("{prefix}/{suffix}")
	})
}

/// `spelled`, the literal part of a re-expressed pattern, unless it names a
/// directory of `from` whose glob metacharacters it would turn into glob
/// syntax. The declared text before the first glob component has none, so
/// any it spells come from `from`.
fn literal(spelled: String, from: &RepoPath) -> Result<String, RebaseError> {
	if has_glob(&spelled) {
		Err(RebaseError::PatternDirectory(from.clone()))
	} else {
		Ok(spelled)
	}
}

/// Whether text contains a glob metacharacter.
fn has_glob(value: &str) -> bool {
	value.contains(['*', '?', '[', ']', '{', '}'])
}

/// A canonical target relative to a canonical directory.
fn relative(target: &RepoPath, directory: &RepoPath) -> String {
	let target: Vec<_> = target.components().collect();
	let directory: Vec<_> = directory.components().collect();
	let common = target
		.iter()
		.zip(&directory)
		.take_while(|(left, right)| left == right)
		.count();
	let mut parts = vec![".."; directory.len() - common];
	parts.extend_from_slice(&target[common..]);
	if parts.is_empty() {
		".".to_owned()
	} else {
		protect_relative(&parts.join("/"))
	}
}

/// Prevents a relative first component from reading as a Windows drive.
fn protect_relative(path: &str) -> String {
	if path.as_bytes().get(1) == Some(&b':')
		&& path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
	{
		format!("./{path}")
	} else {
		path.to_owned()
	}
}

#[cfg(test)]
mod tests;
