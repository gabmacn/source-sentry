//! Expectations: what a reviewer approved, which `attest` binds.
//!
//! `diff` emits the expectations of a document's selected sources, the
//! reviewer approves what `diff` showed, and `attest` refuses unless the
//! snapshot still matches every one of them, pattern membership included.
//!
//! # Written form
//!
//! - `<path>@<oid>`: the file at `path` has the canonical oid `oid`, such as
//!   `src/a.rs@sha1:1234567890abcdef1234567890abcdef12345678`.
//! - `<path>@absent`: `path`, a pattern's attested member, is expected to
//!   have left the pattern, usually because it was deleted. Only a pattern's
//!   membership can acknowledge a removal.
//!
//! `path` is a canonical repository path, written without a leading `/`; a
//! leading `/` is accepted when parsing, since a leading `/` already means
//! the repository root in a source path. The text after the last `@` is the
//! state, so a path may contain `@` itself.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use super::Mismatch;
use crate::model::{ObjectId, OidError, PathError, RepoPath};

/// The word that marks an expected removal.
const ABSENT: &str = "absent";

/// One file's state, as a reviewer approved it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Expectation {
	/// The file exists, with this canonical oid.
	Present {
		/// The file.
		path: RepoPath,
		/// Its canonical blob oid.
		oid: ObjectId,
	},
	/// The file, an attested member of a pattern, has left the pattern.
	Absent {
		/// The former member.
		path: RepoPath,
	},
}

impl Expectation {
	/// The file the expectation is about.
	#[must_use]
	pub fn path(&self) -> &RepoPath {
		match self {
			Self::Present { path, .. } | Self::Absent { path } => path,
		}
	}
}

impl fmt::Display for Expectation {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Present { path, oid } => {
				write!(formatter, "{}@{oid}", path.as_str())
			}
			Self::Absent { path } => write!(formatter, "{}@{ABSENT}", path.as_str()),
		}
	}
}

/// Why an expectation could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExpectationError {
	/// No `@` separates the path from its state.
	#[error(
		"`{0}` has no state; write `<path>@<oid>`, or `<path>@absent` for a \
		 removal"
	)]
	MissingState(String),
	/// The path is not a valid repository path.
	#[error("`{value}`: {error}")]
	Path {
		/// The expectation, as given.
		value: String,
		/// Why the path was rejected.
		error: PathError,
	},
	/// The path names the repository root rather than a file.
	#[error("`{0}` names the repository root rather than a file")]
	Root(String),
	/// The path is not written canonically.
	#[error("`{value}` is not canonical; write the path as `{canonical}`")]
	NotCanonical {
		/// The expectation, as given.
		value: String,
		/// The canonical spelling of its path.
		canonical: String,
	},
	/// The state is neither an oid nor `absent`.
	#[error("`{value}`: {error}; or write `absent` for a removal")]
	State {
		/// The expectation, as given.
		value: String,
		/// Why the oid was rejected.
		error: OidError,
	},
}

impl FromStr for Expectation {
	type Err = ExpectationError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		let (written, state) = value
			.rsplit_once('@')
			.ok_or_else(|| ExpectationError::MissingState(value.to_owned()))?;
		let relative = written.strip_prefix('/').unwrap_or(written);
		let path =
			RepoPath::new(relative).map_err(|error| ExpectationError::Path {
				value: value.to_owned(),
				error,
			})?;
		if path.is_root() {
			return Err(ExpectationError::Root(value.to_owned()));
		}
		if path.as_str() != relative {
			return Err(ExpectationError::NotCanonical {
				value: value.to_owned(),
				canonical: path.as_str().to_owned(),
			});
		}
		if state == ABSENT {
			return Ok(Self::Absent { path });
		}
		let oid = state.parse().map_err(|error| ExpectationError::State {
			value: value.to_owned(),
			error,
		})?;
		Ok(Self::Present { path, oid })
	}
}

/// The expectations that describe a selection: each current file, and each
/// former pattern member that no current source includes, sorted by path.
pub(super) fn describe(
	present: &BTreeMap<RepoPath, ObjectId>,
	removed: &BTreeSet<RepoPath>,
) -> Vec<Expectation> {
	let mut expectations: Vec<Expectation> = present
		.iter()
		.map(|(path, oid)| Expectation::Present {
			path: path.clone(),
			oid: oid.clone(),
		})
		.chain(
			removed
				.iter()
				.filter(|path| !present.contains_key(*path))
				.map(|path| Expectation::Absent { path: path.clone() }),
		)
		.collect();
	expectations.sort_by(|left, right| left.path().cmp(right.path()));
	expectations
}

/// Every difference between `observed`, the snapshot's expectations, and
/// `given`, the approved ones, sorted by path. Empty when they match
/// exactly.
pub(super) fn compare(
	observed: &[Expectation],
	given: &[Expectation],
) -> Vec<Mismatch> {
	let mut mismatches = Vec::new();
	let mut approved: BTreeMap<&RepoPath, &Expectation> = BTreeMap::new();
	let mut repeated = BTreeSet::new();
	for expectation in given {
		if approved.insert(expectation.path(), expectation).is_some() {
			repeated.insert(expectation.path());
		}
	}
	mismatches.extend(
		repeated
			.into_iter()
			.map(|path| Mismatch::Repeated(path.clone())),
	);
	let actual: BTreeMap<&RepoPath, &Expectation> = observed
		.iter()
		.map(|expectation| (expectation.path(), expectation))
		.collect();
	for (path, state) in &actual {
		match approved.get(path) {
			None => mismatches.push(Mismatch::Unexpected((*state).clone())),
			Some(expected) if expected != state => {
				mismatches.push(Mismatch::Differs {
					expected: (*expected).clone(),
					actual: Some((*state).clone()),
				});
			}
			Some(_) => {}
		}
	}
	for (path, expected) in &approved {
		if !actual.contains_key(path) {
			mismatches.push(Mismatch::Differs {
				expected: (*expected).clone(),
				actual: None,
			});
		}
	}
	mismatches
		.sort_by(|left, right| mismatch_path(left).cmp(mismatch_path(right)));
	mismatches
}

/// The path a mismatch is about.
fn mismatch_path(mismatch: &Mismatch) -> &RepoPath {
	match mismatch {
		Mismatch::Unexpected(expectation)
		| Mismatch::Differs {
			expected: expectation,
			..
		} => expectation.path(),
		Mismatch::Repeated(path) => path,
	}
}
