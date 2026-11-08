//! Format adapters: MD/UTF8 frontmatter, opaque files and JSON lockfiles.
//!
//! Every adapter works on bytes the caller has already read, and returns bytes
//! for the caller to write, so the core stays free of I/O.
//!
//! - [`frontmatter`] reads and rewrites the `source-sentry` key of a
//!   document's YAML frontmatter, leaving every other byte as it was.
//! - [`lockfile`] parses and writes local and remote lockfiles.
//! - [`Adapter`] names the two document adapters. Opaque
//!   files carry no declaration, so they need no code of their own here.
//!
//! Both formats share one validation of declarations, whose failures are
//! [`DeclarationError`]s. Every failure in this module is a configuration
//! error.

mod declaration;
pub mod frontmatter;
pub mod lockfile;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use declaration::{DeclarationError, Site};

use crate::model::{Declaration, RepoPath};

/// How Source Sentry reads a document, chosen per file pattern by the config.
///
/// Adapters differ only in whether the format can carry its own declaration.
/// Both kinds are hashed as the blob git would store; hashing belongs to the
/// backend.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
	/// UTF-8 text, such as Markdown, that can carry a declaration in YAML
	/// frontmatter. Written `md-utf8`.
	MdUtf8,
	/// Any other file. It is tracked through lockfiles only, and never read
	/// for a declaration. Written `opaque`.
	Opaque,
}

impl Adapter {
	/// The name used in configs, such as `md-utf8`.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::MdUtf8 => "md-utf8",
			Self::Opaque => "opaque",
		}
	}

	/// Whether documents read through this adapter can be tracked on-file.
	#[must_use]
	pub fn carries_declaration(self) -> bool {
		match self {
			Self::MdUtf8 => true,
			Self::Opaque => false,
		}
	}

	/// Reads the declaration that `document`'s own `bytes` carry, if any.
	///
	/// An opaque document never carries one, so its bytes are not looked at.
	/// An MD/UTF8 document is read by [`frontmatter::read`].
	///
	/// # Errors
	///
	/// Fails as [`frontmatter::read`] does.
	pub fn read_declaration(
		self,
		document: &RepoPath,
		bytes: &[u8],
	) -> Result<Option<Declaration>, frontmatter::FrontmatterError> {
		match self {
			Self::MdUtf8 => frontmatter::read(document, bytes),
			Self::Opaque => Ok(None),
		}
	}
}

impl fmt::Display for Adapter {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.name())
	}
}

/// A position in a file, both counted from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
	/// The line.
	pub line: usize,
	/// The column, in characters.
	pub column: usize,
}

impl fmt::Display for Position {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "line {}, column {}", self.line, self.column)
	}
}

#[cfg(test)]
mod tests;
