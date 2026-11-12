//! Source spellings re-expressed between document directories.

use super::{PathError, RepoPath};

/// Why a source spelling could not be re-expressed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RebaseError {
	/// A source or its literal prefix cannot resolve within the repository.
	#[error(transparent)]
	Path(#[from] PathError),
	/// A parent component after a wildcard has no lexical meaning.
	#[error("`..` follows a wildcard; write the directories out instead")]
	ParentAfterWildcard,
	/// A literal directory would become glob syntax in the new spelling.
	#[error(
		"`{0}` contains glob metacharacters and cannot be a literal pattern prefix"
	)]
	PatternPrefix(String),
}

/// Re-expresses `declared`, a source path written relative to the directory
/// `from`, relative to the directory `to`. A `/`-rooted spelling is returned
/// unchanged.
///
/// # Errors
///
/// Fails when the relative source cannot resolve within the repository.
pub fn rebase_path(
	declared: &str,
	from: &RepoPath,
	to: &RepoPath,
) -> Result<String, RebaseError> {
	if declared.starts_with('/') {
		return Ok(declared.to_owned());
	}
	if declared.is_empty() {
		return Err(PathError::Empty.into());
	}
	Ok(relative(&from.join(declared)?, to))
}

/// The same for a source pattern, rebased by its literal prefix (the leading
/// components with no glob metacharacter).
///
/// # Errors
///
/// Fails when the literal prefix cannot resolve, a parent component follows
/// a wildcard, or the new literal prefix would introduce glob syntax.
pub fn rebase_pattern(
	declared: &str,
	from: &RepoPath,
	to: &RepoPath,
) -> Result<String, RebaseError> {
	if declared.starts_with('/') {
		return Ok(declared.to_owned());
	}
	if declared.is_empty() {
		return Err(PathError::Empty.into());
	}
	// Validate forbidden path characters even in the wildcard suffix.
	from.join(declared)?;
	let components: Vec<&str> = declared.split('/').collect();
	let literal = components.iter().take_while(|part| !wildcard(part)).count();
	let suffix = &components[literal..];
	if suffix.contains(&"..") {
		return Err(RebaseError::ParentAfterWildcard);
	}
	let prefix = relative(&from.join(&components[..literal].join("/"))?, to);
	if wildcard(&prefix) {
		return Err(RebaseError::PatternPrefix(prefix));
	}
	if suffix.is_empty() {
		Ok(prefix)
	} else if prefix == "." {
		Ok(suffix.join("/"))
	} else {
		Ok(format!("{prefix}/{}", suffix.join("/")))
	}
}

/// Whether a component contains glob syntax.
fn wildcard(component: &str) -> bool {
	component.contains(['*', '?', '[', ']', '{', '}'])
}

/// A canonical target relative to a canonical directory.
fn relative(target: &RepoPath, directory: &RepoPath) -> String {
	let target: Vec<&str> = target.components().collect();
	let directory: Vec<&str> = directory.components().collect();
	let common = target
		.iter()
		.zip(&directory)
		.take_while(|(a, b)| a == b)
		.count();
	let parts: Vec<&str> = std::iter::repeat_n("..", directory.len() - common)
		.chain(target[common..].iter().copied())
		.collect();
	if parts.is_empty() {
		".".to_owned()
	} else {
		parts.join("/")
	}
}

#[cfg(test)]
mod tests;
