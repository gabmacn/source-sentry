//! Series lifecycles and version kinds.

use serde::{Deserialize, Serialize};

/// What happens to a series' older entries.
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
pub enum Lifecycle {
	/// The newest entry is the living document; older entries are records.
	Living,
	/// Every entry is a record.
	RecordsOnly,
}

/// How a series' versions are parsed and ordered.
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
pub enum VersionKind {
	/// A whole number, such as `audit-0001`.
	Integer,
	/// Numbers separated by dots, compared component by component, with
	/// trailing zeros equivalent, such as `v0.3` and `v0.3.0`.
	Dotted,
	/// A calendar date, such as `2026-11-07`.
	Date,
}
