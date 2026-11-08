//! Detects files whose committed CRLF line endings make `git hash-object` and
//! `git add` disagree.
//!
//! Under git's "auto" line-ending conversion, `git add` leaves CRLF alone
//! when the index already holds the file with CRLF, while `git hash-object`,
//! which never reads the index, converts it. Measured with git 2.56 on 26
//! combinations of attributes, `core.autocrlf` and contents, the two disagree
//! exactly when all of these hold, which `git ls-files --eol` reports:
//!
//! - the conversion is "auto": `attr/` starts with `text=auto` (from
//!   `text=auto`, with or without `eol`, or the deprecated `crlf=auto`), or
//!   `attr/` is empty and `core.autocrlf` is `true` or `input`. An explicit
//!   `text`, `-text`, `eol` or `text=input` never disagrees;
//! - the index blob is text with CRLF: `i/crlf` or `i/mixed`;
//! - the working-tree file is text with CRLF: `w/crlf` or `w/mixed`.
//!
//! `git ls-files` inspects the working-tree bytes before
//! `working-tree-encoding` decodes them, so for an encoded file `w/` is not
//! meaningful (UTF-16 reads as `-text`). Such a file is reported whenever the
//! first two conditions hold.

use std::collections::HashSet;

use source_sentry_core::ports::{BackendError, BackendErrorKind, RepoPath};

use crate::{
	attributes::{Conversion, State},
	command::{Git, exit_failure},
	error::failure,
};

/// Keeps each `git ls-files` command line well below the 32,767 characters
/// that Windows allows.
const MAX_PATHSPEC_BYTES: usize = 16_000;

/// The value of `core.autocrlf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoCrlf {
	/// Unset or false: no conversion unless attributes ask for one.
	False,
	/// `true`: "auto" conversion for files without line-ending attributes.
	True,
	/// `input`: the same, converting only when files are added.
	Input,
}

/// What a file's attributes say about "auto" line-ending conversion, as far
/// as `git check-attr` can tell. This only narrows the paths that
/// `git ls-files --eol` must inspect; that command decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
	/// `text=auto`, or the deprecated `crlf=auto` where `text` says nothing.
	Auto,
	/// No line-ending attribute applies, so `core.autocrlf` decides.
	Autocrlf,
	/// An explicit setting that never keeps CRLF from the index.
	Never,
}

/// Classifies a path by its `text` and `crlf` attributes, following git's
/// `convert_attrs`: `text` decides when it is set, unset, `input` or `auto`;
/// otherwise `crlf` does; otherwise `eol` would select an explicit
/// conversion, so treating that case as [`Route::Autocrlf`] only asks
/// `ls-files` about a few paths too many.
pub(crate) fn route(conversion: &Conversion) -> Route {
	match (&conversion.text, &conversion.crlf) {
		(State::Value(text), _) if text == "auto" => Route::Auto,
		(State::Set | State::Unset, _) => Route::Never,
		(State::Value(text), _) if text == "input" => Route::Never,
		(_, State::Value(crlf)) if crlf == "auto" => Route::Auto,
		(_, State::Set | State::Unset) => Route::Never,
		(_, State::Value(crlf)) if crlf == "input" => Route::Never,
		_ => Route::Autocrlf,
	}
}

/// Reads `core.autocrlf` as git interprets it: `input`, or a boolean.
///
/// # Errors
///
/// Fails when `git` fails, including for a value that is not a boolean.
pub(crate) fn read_autocrlf(git: &Git) -> Result<AutoCrlf, BackendError> {
	let args = ["config", "--get", "core.autocrlf"];
	let finished = git.run(&args, None)?;
	match finished.status.code() {
		Some(0) => {}
		Some(1) => return Ok(AutoCrlf::False),
		_ => return Err(exit_failure(&args, &finished)),
	}
	if finished.stdout.trim_ascii().eq_ignore_ascii_case(b"input") {
		return Ok(AutoCrlf::Input);
	}
	let boolean =
		git.output(&["config", "--type=bool", "--get", "core.autocrlf"], None)?;
	Ok(if boolean.trim_ascii() == b"true" {
		AutoCrlf::True
	} else {
		AutoCrlf::False
	})
}

/// One line of `git ls-files --eol`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EolInfo {
	/// `i/`: the index blob's line endings.
	index: String,
	/// `w/`: the working-tree file's line endings.
	worktree: String,
	/// `attr/`: the conversion the attributes select.
	attr: String,
}

/// The paths among `candidates` whose canonical oid `git hash-object` would
/// get wrong, because `git add` would keep the index's CRLF line endings.
/// `encoded` lists the candidates with a `working-tree-encoding`.
///
/// # Errors
///
/// Fails when `git` fails or prints something unexpected.
pub(crate) fn kept_index_crlf(
	git: &Git,
	candidates: &[&RepoPath],
	encoded: &HashSet<&RepoPath>,
	autocrlf: AutoCrlf,
) -> Result<HashSet<RepoPath>, BackendError> {
	let mut kept = HashSet::new();
	for chunk in chunks(candidates) {
		let mut args = vec!["--literal-pathspecs", "ls-files", "-z", "--eol", "--"];
		args.extend(chunk.iter().map(|path| path.as_str()));
		let output = git.output(&args, None)?;
		for (path, info) in parse(&output)? {
			if keeps_index_crlf(&info, autocrlf, encoded.contains(&path)) {
				kept.insert(path);
			}
		}
	}
	Ok(kept)
}

/// Splits paths into command lines of bounded length.
fn chunks<'a>(paths: &[&'a RepoPath]) -> Vec<Vec<&'a RepoPath>> {
	let mut chunks: Vec<Vec<&RepoPath>> = Vec::new();
	let mut length = 0;
	for &path in paths {
		let cost = path.as_str().len() + 3;
		if chunks.is_empty() || length + cost > MAX_PATHSPEC_BYTES {
			chunks.push(Vec::new());
			length = 0;
		}
		length += cost;
		if let Some(chunk) = chunks.last_mut() {
			chunk.push(path);
		}
	}
	chunks
}

/// Whether `git add` would keep CRLF that `git hash-object` converts.
fn keeps_index_crlf(info: &EolInfo, autocrlf: AutoCrlf, encoded: bool) -> bool {
	let auto = info.attr.starts_with("text=auto")
		|| (info.attr.is_empty() && autocrlf != AutoCrlf::False);
	let has_crlf = |eol: &str| matches!(eol, "crlf" | "mixed");
	auto && has_crlf(&info.index) && (encoded || has_crlf(&info.worktree))
}

/// Parses `git ls-files -z --eol`: `i/<eol> w/<eol> attr/<attr>\t<path>\0`,
/// padded with spaces. A conflicted path appears once per stage, with the
/// same values.
fn parse(output: &[u8]) -> Result<Vec<(RepoPath, EolInfo)>, BackendError> {
	let unexpected = |record: &[u8]| {
		failure(
			BackendErrorKind::GitExecutable,
			format!(
				"`git ls-files --eol` printed an unexpected record: {}",
				String::from_utf8_lossy(record)
			),
		)
	};
	let mut records = Vec::new();
	for record in output.split(|&byte| byte == 0) {
		if record.is_empty() {
			continue;
		}
		let text = std::str::from_utf8(record).map_err(|_| unexpected(record))?;
		let (meta, path) =
			text.split_once('\t').ok_or_else(|| unexpected(record))?;
		let rest = meta.strip_prefix("i/").ok_or_else(|| unexpected(record))?;
		let (index, rest) =
			rest.split_once(' ').ok_or_else(|| unexpected(record))?;
		let rest = rest
			.trim_start()
			.strip_prefix("w/")
			.ok_or_else(|| unexpected(record))?;
		let (worktree, rest) =
			rest.split_once(' ').ok_or_else(|| unexpected(record))?;
		let attr = rest
			.trim_start()
			.strip_prefix("attr/")
			.ok_or_else(|| unexpected(record))?;
		let path = crate::index::repo_path(path.as_bytes())?;
		records.push((
			path,
			EolInfo {
				index: index.to_owned(),
				worktree: worktree.to_owned(),
				attr: attr.trim_end().to_owned(),
			},
		));
	}
	Ok(records)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn info(index: &str, worktree: &str, attr: &str) -> EolInfo {
		EolInfo {
			index: index.to_owned(),
			worktree: worktree.to_owned(),
			attr: attr.to_owned(),
		}
	}

	fn conversion(text: State, crlf: State) -> Conversion {
		Conversion {
			filter: State::Unspecified,
			working_tree_encoding: State::Unspecified,
			text,
			crlf,
		}
	}

	#[test]
	fn parses_padded_records() {
		let output = b"i/crlf  w/crlf  attr/text=auto        \tf.txt\0\
			i/crlf  w/mixed attr/text=auto eol=crlf\tdir/g.txt\0\
			i/      w/      attr/                 \tnew.txt\0";
		let records = parse(output).unwrap();
		assert_eq!(records[0].1, info("crlf", "crlf", "text=auto"));
		assert_eq!(records[1].0.as_str(), "dir/g.txt");
		assert_eq!(records[1].1, info("crlf", "mixed", "text=auto eol=crlf"));
		assert_eq!(records[2].1, info("", "", ""));
	}

	/// The measured combinations: in each, `true` means that
	/// `git hash-object` and `git add` disagreed.
	#[test]
	fn matches_the_measured_disagreements() {
		use AutoCrlf::{False, Input, True};
		let cases = [
			(info("crlf", "crlf", "text=auto"), False, true),
			(info("crlf", "lf", "text=auto"), False, false),
			(info("lf", "crlf", "text=auto"), False, false),
			(info("mixed", "crlf", "text=auto"), False, true),
			(info("crlf", "mixed", "text=auto"), False, true),
			(info("crlf", "crlf", "text=auto eol=crlf"), False, true),
			(info("crlf", "crlf", "text=auto eol=lf"), False, true),
			(info("crlf", "crlf", "text"), True, false),
			(info("crlf", "crlf", "text eol=lf"), True, false),
			(info("crlf", "crlf", "text eol=crlf"), False, false),
			(info("crlf", "crlf", "-text"), True, false),
			(info("crlf", "crlf", ""), True, true),
			(info("crlf", "crlf", ""), Input, true),
			(info("crlf", "crlf", ""), False, false),
			(info("-text", "crlf", "text=auto"), False, false),
			(info("crlf", "-text", "text=auto"), False, false),
			(info("crlf", "none", "text=auto"), False, false),
		];
		for (info, autocrlf, disagrees) in cases {
			assert_eq!(
				keeps_index_crlf(&info, autocrlf, false),
				disagrees,
				"{info:?} {autocrlf:?}"
			);
		}
	}

	#[test]
	fn trusts_the_index_alone_for_encoded_files() {
		assert!(keeps_index_crlf(
			&info("crlf", "-text", "text=auto"),
			AutoCrlf::False,
			true
		));
		assert!(!keeps_index_crlf(
			&info("lf", "-text", "text=auto"),
			AutoCrlf::False,
			true
		));
	}

	#[test]
	fn routes_by_text_then_crlf() {
		let value = |text: &str| State::Value(text.to_owned());
		assert_eq!(
			route(&conversion(value("auto"), State::Unspecified)),
			Route::Auto
		);
		assert_eq!(route(&conversion(State::Set, value("auto"))), Route::Never);
		assert_eq!(
			route(&conversion(State::Unset, State::Unspecified)),
			Route::Never
		);
		assert_eq!(
			route(&conversion(value("input"), State::Unspecified)),
			Route::Never
		);
		assert_eq!(
			route(&conversion(State::Unspecified, value("auto"))),
			Route::Auto
		);
		assert_eq!(
			route(&conversion(value("bogus"), State::Unspecified)),
			Route::Autocrlf
		);
		assert_eq!(
			route(&conversion(State::Unspecified, State::Unspecified)),
			Route::Autocrlf
		);
	}

	#[test]
	fn bounds_command_lines() {
		let paths: Vec<RepoPath> = (0..3000)
			.map(|number| {
				RepoPath::new(&format!("directory/file{number:05}.txt")).unwrap()
			})
			.collect();
		let borrowed: Vec<&RepoPath> = paths.iter().collect();
		let chunks = chunks(&borrowed);
		assert!(chunks.len() > 1);
		assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), 3000);
		for chunk in &chunks {
			let length: usize =
				chunk.iter().map(|path| path.as_str().len() + 3).sum();
			assert!(length <= MAX_PATHSPEC_BYTES);
		}
	}
}
