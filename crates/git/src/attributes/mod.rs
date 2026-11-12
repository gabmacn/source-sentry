//! Git attributes: the values git applies to a path, and where a value came
//! from.
//!
//! Values come from `git check-attr`, so they are exactly what
//! `git hash-object` uses. Git cannot say which line set a value, so
//! [`locate_filter`] re-reads the attribute files to find it for the
//! unsupported-filter diagnostic. If its reading ever disagrees with git's, it
//! reports the origin as unknown rather than naming a wrong line.

mod origin;
mod wildmatch;

pub(crate) use origin::{OriginContext, locate_filter};

use source_sentry_core::ports::{BackendError, BackendErrorKind, RepoPath};

use crate::{command::Git, error::failure};

/// The state of one attribute for one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
	/// Set without a value, as in `filter`.
	Set,
	/// Unset, as in `-filter`.
	Unset,
	/// Not mentioned, or reset with `!filter`.
	Unspecified,
	/// Set to a value, as in `filter=lfs`.
	Value(String),
}

impl State {
	/// Parses the value `git check-attr` prints. Git prints the same words
	/// for a value that is literally `set`, `unset` or `unspecified`, so such a
	/// value is read as the state it names.
	fn parse(printed: &str) -> Self {
		match printed {
			"set" => Self::Set,
			"unset" => Self::Unset,
			"unspecified" => Self::Unspecified,
			value => Self::Value(value.to_owned()),
		}
	}

	/// The value, when the attribute is set to one.
	pub(crate) fn value(&self) -> Option<&str> {
		match self {
			Self::Value(value) => Some(value),
			_ => None,
		}
	}
}

/// The attributes that decide how a working-tree file is canonicalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Conversion {
	/// The `filter` attribute: the clean filter driver.
	pub filter: State,
	/// The `working-tree-encoding` attribute, which git applies after a
	/// filter.
	pub working_tree_encoding: State,
	/// The `text` attribute, which decides line-ending normalization.
	pub text: State,
	/// The deprecated `crlf` attribute, which git consults when `text` says
	/// nothing.
	pub crlf: State,
}

/// The attributes [`check`] asks for, in the order of [`Conversion`]'s
/// fields.
const ATTRIBUTES: [&str; 4] =
	["filter", "working-tree-encoding", "text", "crlf"];

/// The attributes git checks in for each path, in the order given.
///
/// # Errors
///
/// Fails when `git check-attr` fails or prints something unexpected.
pub(crate) fn check(
	git: &Git,
	paths: &[&RepoPath],
) -> Result<Vec<Conversion>, BackendError> {
	if paths.is_empty() {
		return Ok(Vec::new());
	}
	let mut input = Vec::new();
	for path in paths {
		input.extend_from_slice(path.as_str().as_bytes());
		input.push(0);
	}
	let mut args = vec!["check-attr", "-z", "--stdin"];
	args.extend(ATTRIBUTES);
	let output = git.output(&args, Some(&input))?;
	parse(&output, paths)
}

/// Parses `git check-attr -z` output: `<path>\0<attribute>\0<info>\0` for
/// each path and attribute, in input order.
fn parse(
	output: &[u8],
	paths: &[&RepoPath],
) -> Result<Vec<Conversion>, BackendError> {
	let unexpected = || {
		failure(
			BackendErrorKind::GitExecutable,
			"`git check-attr` printed something unexpected",
		)
	};
	let mut fields = output.split(|&byte| byte == 0);
	let mut next_state =
		|path: &RepoPath, attribute: &str| -> Result<State, BackendError> {
			let (Some(printed_path), Some(printed_attribute), Some(info)) =
				(fields.next(), fields.next(), fields.next())
			else {
				return Err(unexpected());
			};
			if printed_path != path.as_str().as_bytes()
				|| printed_attribute != attribute.as_bytes()
			{
				return Err(unexpected());
			}
			let info = std::str::from_utf8(info).map_err(|_| unexpected())?;
			Ok(State::parse(info))
		};
	paths
		.iter()
		.map(|path| {
			let [filter, working_tree_encoding, text, crlf] = ATTRIBUTES;
			Ok(Conversion {
				filter: next_state(path, filter)?,
				working_tree_encoding: next_state(path, working_tree_encoding)?,
				text: next_state(path, text)?,
				crlf: next_state(path, crlf)?,
			})
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_check_attr_output() {
		let a = RepoPath::new("a.dat").unwrap();
		let b = RepoPath::new("b.txt").unwrap();
		let output =
			b"a.dat\0filter\0lfs\0a.dat\0working-tree-encoding\0unspecified\0\
			a.dat\0text\0unset\0a.dat\0crlf\0unspecified\0\
			b.txt\0filter\0unset\0b.txt\0working-tree-encoding\0UTF-16\0\
			b.txt\0text\0auto\0b.txt\0crlf\0unspecified\0";
		let parsed = parse(output, &[&a, &b]).unwrap();
		assert_eq!(parsed[0].filter, State::Value("lfs".to_owned()));
		assert_eq!(parsed[0].working_tree_encoding, State::Unspecified);
		assert_eq!(parsed[1].filter, State::Unset);
		assert_eq!(parsed[1].working_tree_encoding.value(), Some("UTF-16"));
		assert_eq!(parsed[0].text, State::Unset);
		assert_eq!(parsed[1].text.value(), Some("auto"));
	}

	#[test]
	fn rejects_misaligned_output() {
		let a = RepoPath::new("a.dat").unwrap();
		assert!(parse(b"b.dat\0filter\0lfs\0", &[&a]).is_err());
		assert!(parse(b"a.dat\0filter\0", &[&a]).is_err());
	}
}
