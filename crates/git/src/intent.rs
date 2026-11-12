//! Marks the files a plan creates intent-to-add.
//!
//! `git add --intent-to-add` records a path in the index without its
//! content, so the next snapshot, whose membership comes from the index,
//! sees the file, and the user stages and commits it as usual.
//! It is the only index change Source Sentry makes.
//!
//! Every command reads its paths from standard input, separated by NUL
//! bytes, so no command line grows with the plan, and `--literal-pathspecs`
//! keeps a name such as `a[1].lock` from being read as a pattern.
//!
//! Measured with git 2.56, a fatal failure such as a held `index.lock`
//! changes nothing, but two inputs make `git add` record some paths and still
//! exit with an error: an ignored path, and a path outside the sparse-checkout
//! definition. [`ignored`] and [`outside_sparse_checkout`] find those before
//! anything is written, so the plan can be refused instead.

use source_sentry_core::ports::{BackendError, RepoPath};

use crate::{
	command::{Git, exit_failure},
	index::repo_path,
};

/// Marks `paths`, which must exist in the working tree and be absent from the
/// index, intent-to-add, in one command.
///
/// # Errors
///
/// Fails when `git add` fails. The index may then hold some of the paths,
/// which [`unmark`] removes again.
pub(crate) fn mark(git: &Git, paths: &[RepoPath]) -> Result<(), BackendError> {
	git
		.output(
			&[
				"--literal-pathspecs",
				"add",
				"--intent-to-add",
				"--pathspec-from-file=-",
				"--pathspec-file-nul",
			],
			Some(&nul_separated(paths)),
		)
		.map(drop)
}

/// Removes `paths` from the index again, leaving the working tree alone.
/// Paths the index does not hold are skipped.
///
/// # Errors
///
/// Fails when `git rm` fails.
pub(crate) fn unmark(
	git: &Git,
	paths: &[RepoPath],
) -> Result<(), BackendError> {
	git
		.output(
			&[
				"--literal-pathspecs",
				"rm",
				"--cached",
				"--quiet",
				"--ignore-unmatch",
				"--pathspec-from-file=-",
				"--pathspec-file-nul",
			],
			Some(&nul_separated(paths)),
		)
		.map(drop)
}

/// The paths among `paths` that a `.gitignore` rule, `info/exclude` or
/// `core.excludesFile` ignores. `git add` refuses them without `--force`.
///
/// # Errors
///
/// Fails when `git check-ignore` fails or prints a path it was not given.
pub(crate) fn ignored(
	git: &Git,
	paths: &[RepoPath],
) -> Result<Vec<RepoPath>, BackendError> {
	let args = ["check-ignore", "-z", "--stdin"];
	let finished = git.run(&args, Some(&nul_separated(paths)))?;
	match finished.status.code() {
		Some(0) => parse_paths(&finished.stdout),
		// Git exits with 1 when no path is ignored.
		Some(1) => Ok(Vec::new()),
		_ => Err(exit_failure(&args, &finished)),
	}
}

/// The paths among `paths` that lie outside the sparse-checkout definition.
/// Call it only when `core.sparseCheckout` is set.
///
/// # Errors
///
/// Fails when `git sparse-checkout check-rules`, which git 2.42 introduced,
/// fails.
pub(crate) fn outside_sparse_checkout(
	git: &Git,
	paths: &[RepoPath],
) -> Result<Vec<RepoPath>, BackendError> {
	let output = git.output(
		&["sparse-checkout", "check-rules", "-z"],
		Some(&nul_separated(paths)),
	)?;
	let inside = parse_paths(&output)?;
	Ok(
		paths
			.iter()
			.filter(|path| !inside.contains(path))
			.cloned()
			.collect(),
	)
}

/// `paths`, each followed by a NUL byte.
fn nul_separated(paths: &[RepoPath]) -> Vec<u8> {
	let mut input = Vec::new();
	for path in paths {
		input.extend_from_slice(path.as_str().as_bytes());
		input.push(0);
	}
	input
}

/// Parses NUL-separated paths.
fn parse_paths(output: &[u8]) -> Result<Vec<RepoPath>, BackendError> {
	output
		.split(|&byte| byte == 0)
		.filter(|path| !path.is_empty())
		.map(repo_path)
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn separates_paths_with_nul_bytes() {
		let paths = [
			RepoPath::new("a[1].lock").unwrap(),
			RepoPath::new("dir/b c.lock").unwrap(),
		];
		let input = nul_separated(&paths);
		assert_eq!(input, b"a[1].lock\0dir/b c.lock\0");
		assert_eq!(parse_paths(&input).unwrap(), paths);
		assert_eq!(parse_paths(b"").unwrap(), Vec::<RepoPath>::new());
	}
}
