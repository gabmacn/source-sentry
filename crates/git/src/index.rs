//! The index's membership, as `git ls-files` reports it.
//!
//! The index is read through the `git` executable rather than `git2`, because
//! libgit2 1.9 rejects a sparse index (`sdir`) and a split index (`link`) as
//! unsupported mandatory extensions, and a sparse checkout is exactly what
//! snapshots must support.

use std::collections::BTreeMap;

use source_sentry_core::ports::{
	BackendError, BackendErrorKind, HashAlgorithm, ObjectId, RepoPath,
};

use crate::{
	command::Git,
	error::{caused, failure},
	ids::parse_printed,
};

/// The file mode of a symbolic link.
pub(crate) const MODE_SYMLINK: u32 = 0o120_000;
/// The file mode of a submodule (gitlink).
pub(crate) const MODE_GITLINK: u32 = 0o160_000;
/// The mask that selects an entry's type from its mode.
const MODE_TYPE_MASK: u32 = 0o170_000;
/// The type bits of a regular file.
const MODE_REGULAR: u32 = 0o100_000;
/// The type bits of a directory, which a sparse index stores for a whole
/// directory outside the sparse cone.
const MODE_DIRECTORY: u32 = 0o040_000;

/// One path of the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexRecord {
	/// The path.
	pub path: RepoPath,
	/// The file mode, such as `0o100644`.
	pub mode: u32,
	/// The staged blob. For a conflicted path this is one stage's blob.
	pub oid: ObjectId,
	/// Whether sparse checkout marks the path skip-worktree, so that the
	/// working tree omits it and the index blob stands for it.
	pub skip_worktree: bool,
}

impl IndexRecord {
	/// Whether the entry is a submodule rather than a file.
	pub(crate) fn is_gitlink(&self) -> bool {
		self.mode & MODE_TYPE_MASK == MODE_GITLINK
	}

	/// Whether the entry is a symbolic link. Under `core.symlinks=false` its
	/// working-tree file is a plain-file placeholder holding the target.
	pub(crate) fn is_symlink(&self) -> bool {
		self.mode & MODE_TYPE_MASK == MODE_SYMLINK
	}
}

/// Lists the index, keyed by path.
///
/// # Errors
///
/// Fails when `git` fails, prints something unexpected, or lists a path that
/// is not a canonical repository path.
pub(crate) fn read_index(
	git: &Git,
	algorithm: HashAlgorithm,
) -> Result<BTreeMap<RepoPath, IndexRecord>, BackendError> {
	// `-t` tags skip-worktree entries with `S`. Without `--sparse`, a sparse
	// index is expanded in memory, so every file is listed. The advice would
	// only repeat that expansion on standard error.
	let output = git.output(
		&[
			"-c",
			"advice.sparseIndexExpanded=false",
			"ls-files",
			"-z",
			"--stage",
			"-t",
		],
		None,
	)?;
	parse_ls_files(&output, algorithm)
}

/// Parses the output of `git ls-files -z --stage -t`: records of the form
/// `<tag> <mode> <oid> <stage>\t<path>\0`.
fn parse_ls_files(
	output: &[u8],
	algorithm: HashAlgorithm,
) -> Result<BTreeMap<RepoPath, IndexRecord>, BackendError> {
	let mut records = BTreeMap::new();
	for record in output.split(|&byte| byte == 0) {
		if record.is_empty() {
			continue;
		}
		let malformed = || {
			failure(
				BackendErrorKind::GitExecutable,
				format!(
					"`git ls-files` printed an unexpected record: {}",
					String::from_utf8_lossy(record)
				),
			)
		};
		let tab = record
			.iter()
			.position(|&byte| byte == b'\t')
			.ok_or_else(malformed)?;
		let (meta, raw_path) = (&record[..tab], &record[tab + 1..]);
		let meta = std::str::from_utf8(meta).map_err(|_| malformed())?;
		let mut fields = meta.split(' ');
		let (Some(tag), Some(mode), Some(oid), Some(_stage), None) = (
			fields.next(),
			fields.next(),
			fields.next(),
			fields.next(),
			fields.next(),
		) else {
			return Err(malformed());
		};
		let mode = u32::from_str_radix(mode, 8).map_err(|_| malformed())?;
		if mode & MODE_TYPE_MASK == MODE_DIRECTORY {
			// Only `--sparse` lists sparse directories, but a directory is
			// never a file either way.
			continue;
		}
		let path = repo_path(raw_path)?;
		let record = IndexRecord {
			path: path.clone(),
			mode,
			oid: parse_printed(oid, algorithm)?,
			skip_worktree: tag == "S",
		};
		// A conflicted path is listed once per stage; one record is enough.
		records.entry(path).or_insert(record);
	}
	Ok(records)
}

/// Whether `mode` is a regular file, executable or not.
pub(crate) fn is_regular(mode: u32) -> bool {
	mode & MODE_TYPE_MASK == MODE_REGULAR
}

/// Converts a path that git printed or stored into a [`RepoPath`].
///
/// # Errors
///
/// Fails with [`BackendErrorKind::Git`] when the path is not UTF-8 or is not
/// already canonical, for example because it contains a backslash. Such a path
/// is reported rather than skipped, because skipping it could hide a source
/// from a pattern.
pub(crate) fn repo_path(raw: &[u8]) -> Result<RepoPath, BackendError> {
	let text = std::str::from_utf8(raw).map_err(|error| {
		caused(
			BackendErrorKind::Git,
			format!(
				"the repository tracks `{}`, whose name is not UTF-8; Source \
				 Sentry cannot represent it",
				String::from_utf8_lossy(raw)
			),
			error,
		)
	})?;
	match RepoPath::new(text) {
		Ok(path) if path.as_str() == text => Ok(path),
		Ok(_) => Err(failure(
			BackendErrorKind::Git,
			format!(
				"the repository tracks `{text}`, which is not a canonical \
				 repository path"
			),
		)),
		Err(error) => Err(caused(
			BackendErrorKind::Git,
			format!(
				"the repository tracks `{text}`, which Source Sentry cannot represent"
			),
			error,
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const OID: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

	#[test]
	fn parses_tags_modes_and_stages() {
		let output = format!(
			"H 100644 {OID} 0\ta.txt\0S 100755 {OID} 0\tdir/b.sh\0\
			 M 100644 {OID} 1\tc.txt\0M 100644 {OID} 2\tc.txt\0\
			 H 160000 {OID} 0\tsub\0"
		);
		let records =
			parse_ls_files(output.as_bytes(), HashAlgorithm::Sha1).unwrap();
		let paths: Vec<&str> = records.keys().map(RepoPath::as_str).collect();
		assert_eq!(paths, ["a.txt", "c.txt", "dir/b.sh", "sub"]);
		let script = &records[&RepoPath::new("dir/b.sh").unwrap()];
		assert!(script.skip_worktree);
		assert!(is_regular(script.mode));
		assert!(records[&RepoPath::new("sub").unwrap()].is_gitlink());
	}

	#[test]
	fn skips_sparse_directories() {
		let output = format!("S 040000 {OID} 0\tother/\0H 100644 {OID} 0\tr.txt\0");
		let records =
			parse_ls_files(output.as_bytes(), HashAlgorithm::Sha1).unwrap();
		assert_eq!(records.len(), 1);
	}

	#[test]
	fn rejects_unrepresentable_paths() {
		assert!(repo_path(b"a\\b").is_err());
		assert!(repo_path(b"\xff").is_err());
		assert!(repo_path(b"a/../b").is_err());
		assert_eq!(repo_path(b"a/b").unwrap().as_str(), "a/b");
	}
}
