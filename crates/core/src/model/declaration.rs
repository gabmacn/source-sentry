//! Declarations, as written in frontmatter and in lockfiles.
//!
//! One set of types serves both formats, because a lockfile entry has exactly
//! the fields of an on-file declaration plus `seal` and `completed`. Fields are
//! declared in alphabetical order, which is the order lockfiles serialize them
//! in. Which fields may appear where is validated by the adapters.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use serde::{Deserialize, Serialize};

use super::{ClassName, ObjectId};

/// When something was reviewed, and by whom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Review {
	/// The moment, in UTC, to the second.
	pub at: DateTime<Utc>,
	/// The reviewer's name, or `None` when nobody could be identified. It is
	/// written as `null`, never omitted.
	pub by: Option<String>,
}

impl Review {
	/// A review at `at`, truncated to the second, by `by`.
	#[must_use]
	pub fn new(at: DateTime<Utc>, by: Option<String>) -> Self {
		Self {
			at: at.trunc_subsecs(0),
			by,
		}
	}
}

/// A record's seal: its canonical blob oid, and who sealed it when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Seal {
	/// When the record was sealed.
	pub at: DateTime<Utc>,
	/// Who sealed it, or `None` when nobody could be identified.
	pub by: Option<String>,
	/// The record's canonical blob oid at sealing.
	pub oid: ObjectId,
}

/// A schedule: a grace period and an optional cadence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Schedule {
	/// The cadence that due dates fall on: inherited when absent, removed by
	/// `null`, or set.
	#[serde(default, skip_serializing_if = "CadenceSetting::is_inherit")]
	pub cadence: CadenceSetting,
	/// Whole days a document may lag behind a changed source. When absent, the
	/// inherited value applies.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub grace: Option<u32>,
}

/// What a schedule says about its cadence.
///
/// Written as a missing key, `null` or a cadence. `null` is the one empty value
/// with a meaning: it removes a cadence that would otherwise be inherited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CadenceSetting {
	/// The key is absent, so an inherited cadence applies.
	#[default]
	Inherit,
	/// `cadence: null`: no cadence, even if one would be inherited.
	Remove,
	/// This cadence.
	Set(Cadence),
}

impl CadenceSetting {
	/// Whether the key is absent, so serialization omits it.
	#[must_use]
	pub fn is_inherit(&self) -> bool {
		matches!(self, Self::Inherit)
	}

	/// The cadence, when this setting sets one.
	#[must_use]
	pub fn as_set(&self) -> Option<&Cadence> {
		match self {
			Self::Set(cadence) => Some(cadence),
			Self::Inherit | Self::Remove => None,
		}
	}
}

impl Serialize for CadenceSetting {
	fn serialize<S: serde::Serializer>(
		&self,
		serializer: S,
	) -> Result<S::Ok, S::Error> {
		self.as_set().serialize(serializer)
	}
}

impl<'de> Deserialize<'de> for CadenceSetting {
	/// Only called when the key is present, so `null` means removal; an
	/// absent key falls back to [`CadenceSetting::Inherit`] through
	/// `#[serde(default)]`.
	fn deserialize<D: serde::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		Ok(
			Option::<Cadence>::deserialize(deserializer)?
				.map_or(Self::Remove, Self::Set),
		)
	}
}

/// A calendar cadence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Cadence {
	/// The first due date. Required, and date-only.
	pub dtstart: NaiveDate,
	/// An RFC 5545 recurrence rule without time-of-day parts, `COUNT` or
	/// `UNTIL`.
	pub rrule: String,
}

/// One declared source: a single path, or a pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SourceDecl {
	/// A single source path.
	Path(PathSource),
	/// A pattern whose every match is a source.
	Pattern(PatternSource),
}

impl SourceDecl {
	/// The path or pattern as declared, which lockfiles sort sources by.
	#[must_use]
	pub fn declared(&self) -> &str {
		match self {
			Self::Path(source) => &source.path,
			Self::Pattern(source) => &source.pattern,
		}
	}
}

/// A single source path, attested or not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PathSource {
	/// The attested canonical blob oid. Present exactly when `reviewed` is.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub oid: Option<ObjectId>,
	/// The path as declared, in the syntax of source paths.
	pub path: String,
	/// Who confirmed this source, and when. Present exactly when `oid` is.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reviewed: Option<Review>,
}

/// A pattern source and its attested members.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PatternSource {
	/// The members as last attested, sorted by path.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub members: Vec<Member>,
	/// The glob, as declared.
	pub pattern: String,
}

/// One attested member of a pattern source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Member {
	/// The attested canonical blob oid.
	pub oid: ObjectId,
	/// The member's path, in the syntax of source paths.
	pub path: String,
	/// Who confirmed this member, and when.
	pub reviewed: Review,
}

/// A document's declaration: the `source-sentry` key of its frontmatter, or
/// its entry in a lockfile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Declaration {
	/// Classes assigned to the document.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub classes: Vec<ClassName>,
	/// When a `living` series entry first became fully attested.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completed: Option<Review>,
	/// Owner identifiers.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub owners: Vec<String>,
	/// The last review of the whole document.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reviewed: Option<Review>,
	/// A schedule that overrides the inherited one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub schedule: Option<Schedule>,
	/// The record's seal. Lockfiles only.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub seal: Option<Seal>,
	/// The declared sources. Lockfiles sort them by their declared path or
	/// pattern; frontmatter keeps the author's order.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub sources: Vec<SourceDecl>,
	/// A link for people to follow. Remote documents only, and never part of
	/// the identity.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub url: Option<String>,
}

/// A tombstone left by `retire`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Retired {
	/// The completion, kept when the document was a `living` series entry
	/// that had completed, so that the window it credited stays credited.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completed: Option<Review>,
	/// Who retired the document, and when.
	pub retired: Review,
	/// The seal, kept when the document was a record.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub seal: Option<Seal>,
}

/// A whole lockfile, local or remote.
///
/// Keys are kept as written: paths relative to the lockfile's directory in a
/// local lockfile, identities in a remote one. The adapters parse and
/// validate them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Lockfile {
	/// The tracked documents.
	pub documents: BTreeMap<String, Declaration>,
	/// Non-empty lists of tombstones, oldest retirement first. Equal
	/// retirement instants retain their written order.
	pub retired: BTreeMap<String, Vec<Retired>>,
	/// The version of the lockfile format.
	pub schema: u32,
}
