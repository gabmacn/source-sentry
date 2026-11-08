//! The MD/UTF8 adapter: a document's own declaration, under the `source-sentry`
//! key of its YAML frontmatter.
//!
//! Source Sentry owns only that key. Other tools' keys are never parsed, so
//! no value in them can make reading fail, and writing changes nothing but
//! the key's own lines: other keys, comments, the body, the byte order mark
//! and the line endings stay exactly as they were.
//!
//! The key's value is parsed strictly: YAML 1.1 booleans, unknown tags, merge
//! keys and duplicate keys are rejected, so `*.md` fails as an unknown alias
//! and `!draft x` fails as a tag instead of reading as `x`. `scan` documents
//! how the frontmatter and the key are located.

mod emit;
mod scan;

use std::ops::Range;

use serde::Deserialize;
use serde_saphyr::{MessageFormatter as _, UserMessageFormatter};

use super::{
	DeclarationError, Position,
	declaration::{FieldPath, RawDeclaration, Scope, validate},
};
use crate::model::{Declaration, RepoPath};
use scan::Located;

/// Why a document's frontmatter could not be read or written. Line numbers
/// count from 1, with the opening `---` on line 1.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrontmatterError {
	/// The frontmatter has the `source-sentry` key more than once.
	#[error(
		"the frontmatter has `source-sentry` on lines {first} and {second}; a \
		 document has one declaration"
	)]
	DuplicateKey {
		/// The first key's line.
		first: usize,
		/// The second key's line.
		second: usize,
	},
	/// A line mentions `source-sentry` in a form that cannot be located by
	/// line, such as a flow mapping, an explicit `? ` key, or frontmatter
	/// that is not a block mapping at column 0.
	#[error(
		"line {line} mentions `source-sentry`, but not as a key Source Sentry \
		 can locate; write `source-sentry:` at the start of a line"
	)]
	Unlocatable {
		/// The line.
		line: usize,
	},
	/// A declaration cannot be added to this document.
	#[error("cannot add a declaration (line {line}): {reason}")]
	CannotInsert {
		/// The line that prevents it.
		line: usize,
		/// Why.
		reason: &'static str,
	},
	/// The `source-sentry` block is not valid UTF-8.
	#[error("the `source-sentry` block on line {line} is not valid UTF-8")]
	NotUtf8 {
		/// The key's line.
		line: usize,
	},
	/// `source-sentry` is present, but empty or `null`.
	#[error(
		"`source-sentry` on line {line} has no value; give it a mapping, or \
		 remove the key"
	)]
	NoValue {
		/// The key's line.
		line: usize,
	},
	/// The block is not valid YAML, breaks the strict parser options, or does
	/// not match the declaration's fields.
	#[error("{}{message}", prefix(.position.as_ref(), .at))]
	Yaml {
		/// Where the problem is, when the parser knows.
		position: Option<Position>,
		/// The field, such as `source-sentry.sources[0].oid`, or empty when
		/// the problem is not inside one.
		at: String,
		/// What is wrong.
		message: String,
	},
	/// The declaration parsed, but breaks a rule of the record.
	#[error(transparent)]
	Declaration(#[from] DeclarationError),
}

/// The key Source Sentry owns.
const KEY: &str = "source-sentry";

/// `line N, column M: `field`: `, leaving out what is unknown.
fn prefix(position: Option<&Position>, at: &str) -> String {
	let mut prefix =
		position.map_or_else(String::new, |position| format!("{position}: "));
	if !at.is_empty() {
		prefix = format!("{prefix}`{at}`: ");
	}
	prefix
}

/// Reads the declaration in `document`'s frontmatter.
///
/// Returns `None` when there is no frontmatter, when it has no `source-sentry`
/// key, or when it is not a YAML block mapping and does not mention the key.
/// `document` is the document's own path, which relative source paths resolve
/// against.
///
/// # Errors
///
/// Fails when the key appears twice or cannot be located (see
/// [`FrontmatterError`]), when its value is empty, not valid UTF-8 or not a
/// valid declaration under the strict options, or when the declaration
/// breaks a rule: `seal` and `url` are not allowed, a path source has `oid`
/// and `reviewed` together or neither, and every source path must resolve.
pub fn read(
	document: &RepoPath,
	bytes: &[u8],
) -> Result<Option<Declaration>, FrontmatterError> {
	match scan::locate(bytes)? {
		Located::Present { block, line, .. } => {
			parse(document, bytes, block, line).map(Some)
		}
		Located::NoFrontmatter { .. }
		| Located::NotAMapping { .. }
		| Located::Absent { .. } => Ok(None),
	}
}

/// Returns `bytes` with the `source-sentry` key set to `declaration`, or
/// removed when `declaration` is `None`.
///
/// Only the key's own lines change. A missing key is added at the end of the
/// frontmatter, and a document without frontmatter gets one at the start,
/// after any byte order mark. New lines use the line ending of the opening
/// `---`, or of the file's first line when there is no frontmatter yet.
/// Removing the last key leaves the empty `---` pair in place, because those
/// bytes are not the key's.
///
/// When the document already holds an equal declaration, `bytes` are
/// returned unchanged, comments in the block included. Otherwise the block is
/// rewritten in the house style of `structure/example.yaml`, and comments
/// inside it are lost. The result is deterministic, and [`read`] returns
/// `declaration` from it.
///
/// # Errors
///
/// Fails when `declaration` breaks a rule of [`read`]; when the existing
/// frontmatter cannot be read, because a declaration that cannot be read is
/// never overwritten; and when a declaration must be added to frontmatter
/// that is not a block mapping, or in front of TOML frontmatter.
pub fn write(
	document: &RepoPath,
	bytes: &[u8],
	declaration: Option<&Declaration>,
) -> Result<Vec<u8>, FrontmatterError> {
	if let Some(declaration) = declaration {
		validate(
			declaration,
			Scope::Frontmatter(document),
			&FieldPath::root(KEY),
		)?;
	}
	let (range, replacement) = match (scan::locate(bytes)?, declaration) {
		(
			Located::NoFrontmatter { .. }
			| Located::NotAMapping { .. }
			| Located::Absent { .. },
			None,
		) => return Ok(bytes.to_vec()),
		(Located::NoFrontmatter { toml: true, .. }, Some(_)) => {
			return Err(FrontmatterError::CannotInsert {
				line: 1,
				reason: "the document starts with TOML frontmatter (`+++`)",
			});
		}
		(Located::NoFrontmatter { insert_at, eol, .. }, Some(declaration)) => {
			let eol_str = eol.as_str();
			let frontmatter =
				format!("---{eol_str}{}---{eol_str}", emit::block(declaration, eol));
			(insert_at..insert_at, frontmatter)
		}
		(Located::NotAMapping { line }, Some(_)) => {
			return Err(FrontmatterError::CannotInsert {
				line,
				reason: "the frontmatter is not a YAML block mapping",
			});
		}
		(Located::Absent { insert_at, eol }, Some(declaration)) => {
			(insert_at..insert_at, emit::block(declaration, eol))
		}
		(Located::Present { block, line, eol }, declaration) => {
			let existing = parse(document, bytes, block.clone(), line)?;
			match declaration {
				Some(declaration) if *declaration == existing => {
					return Ok(bytes.to_vec());
				}
				Some(declaration) => (block, emit::block(declaration, eol)),
				None => (block, String::new()),
			}
		}
	};
	let mut written = Vec::with_capacity(bytes.len() + replacement.len());
	written.extend_from_slice(&bytes[..range.start]);
	written.extend_from_slice(replacement.as_bytes());
	written.extend_from_slice(&bytes[range.end..]);
	Ok(written)
}

/// Parses and validates the block at `range`, whose key is on `line`.
fn parse(
	document: &RepoPath,
	bytes: &[u8],
	range: Range<usize>,
	line: usize,
) -> Result<Declaration, FrontmatterError> {
	/// The block, which holds only the key.
	#[derive(Deserialize)]
	#[serde(deny_unknown_fields)]
	struct Block {
		#[serde(rename = "source-sentry")]
		declaration: Option<RawDeclaration>,
	}

	let text = std::str::from_utf8(&bytes[range])
		.map_err(|_| FrontmatterError::NotUtf8 { line })?;
	// Blank lines in front of the block make the parser's line numbers the
	// file's, and change nothing else.
	let padded = format!("{}{text}", "\n".repeat(line.saturating_sub(1)));
	let mut at = FieldPath::default();
	let block = serde_saphyr::with_deserializer_from_str_with_options(
		&padded,
		options(),
		|deserializer| {
			serde_path_to_error::deserialize::<_, Block>(deserializer).map_err(
				|error| {
					at = FieldPath::from_serde(error.path());
					error.into_inner()
				},
			)
		},
	)
	.map_err(|error| yaml_error(&error, at))?;
	let declaration = Declaration::from(
		block
			.declaration
			.ok_or(FrontmatterError::NoValue { line })?,
	);
	validate(
		&declaration,
		Scope::Frontmatter(document),
		&FieldPath::root(KEY),
	)?;
	Ok(declaration)
}

/// The strict parser options.
fn options() -> serde_saphyr::Options {
	crate::yaml::strict_options()
}

/// Converts a parser error at the field `at`, adding the correction for the two
/// hazards that quoting fixes.
fn yaml_error(error: &serde_saphyr::Error, at: FieldPath) -> FrontmatterError {
	let error = error.without_snippet();
	let position =
		error
			.location()
			.filter(|location| location.line() > 0)
			.map(|location| Position {
				line: to_usize(location.line()),
				column: to_usize(location.column()),
			});
	let mut message = sanitize(&UserMessageFormatter.format_message(error));
	let hint = match error {
		serde_saphyr::Error::UnknownAnchor { .. } => Some(
			"a value that starts with `*` is read as a YAML alias; quote it, for \
			 example `'*.md'`",
		),
		serde_saphyr::Error::UnsupportedTag { .. } => Some(
			"a value that starts with `!` is read as a YAML tag; quote it, for \
			 example `'!draft'`",
		),
		_ => None,
	};
	if let Some(hint) = hint {
		message = format!("{message}; {hint}");
	}
	FrontmatterError::Yaml {
		position,
		at: at.into_string(),
		message,
	}
}

/// A parser coordinate as a `usize`. Coordinates count positions in a string
/// held in memory, so they always fit.
fn to_usize(coordinate: u64) -> usize {
	usize::try_from(coordinate).unwrap_or(usize::MAX)
}

/// Escapes control characters in a parser message, which may quote the
/// document.
fn sanitize(message: &str) -> String {
	let mut clean = String::with_capacity(message.len());
	for character in message.chars() {
		if character.is_control() {
			clean.extend(character.escape_debug());
		} else {
			clean.push(character);
		}
	}
	clean
}

#[cfg(test)]
mod tests;
