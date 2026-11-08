//! The report model, with reasons and origins.
//!
//! The evaluation's types carry values from several modules that are not
//! serializable, so this module defines the JSON shapes that tools read, and
//! builds them from the model without changing it:
//!
//! - [`CheckReport`] is the output of `check --format json`, shaped as in
//!   `structure/report.example.json`;
//! - [`DiffReport`] is the output of `diff --format json`, in the same style.
//!
//! # Conventions
//!
//! Every shape follows these conventions:
//!
//! - **Keys** are kebab-case, and sorted alphabetically at every level.
//!   [`to_json`] serializes through [`serde_json::Value`], whose objects sort
//!   their keys.
//! - **`schema`** versions each format, starting at [`SCHEMA`]. A change that
//!   removes or renames a key, or changes a value's meaning, needs a new
//!   schema.
//! - **Every key that the example shows is always present.** When its value
//!   is unknown or does not apply, it is `null`: a fallback's `rule`, a
//!   cadence that is not set, a location without a line.
//! - **Keys for situations the example does not show** appear only in those
//!   situations, so the example's subjects keep exactly its keys. A document
//!   gains `unattested` when it has never been reviewed, `record` when it is
//!   a sealed record, `awaiting-seal` when a series successor exists, and
//!   `stale-upstreams` when a source document is stale. A source gains
//!   `pattern` when it is a pattern's member. A revision of kind `commit`,
//!   from `--rev`, has `commit` where the working tree has `head`.
//! - **Clocks** are `{"commit", "since"}` once started, `null` when none
//!   runs, and otherwise `not-started`, `unknown` or `failed`; see
//!   [`ClockReport`].
//! - **Judgements** are `null`, with an `outcome` of `null`, for a subject
//!   that was not judged: when no context could be selected, or when an
//!   error stands in for a record's outcome.
//! - **Instants** are RFC 3339 in UTC with a `Z`, and **dates** are
//!   `YYYY-MM-DD`. Object IDs carry their algorithm, as in lockfiles.
//! - **Directories** are repository paths, and the repository root is `.`.
//! - **Scopes** are written `repository`, `directory:<path>` or
//!   `remote:<owner>`, like `track`'s placements.
//!
//! # Origins
//!
//! An origin says where a setting or a rule comes from.
//! It has a `kind`:
//!
//! - `config`: the `file`, the `line` (or `null`), the `import-chain` of
//!   presets through which the file was imported, and the `scope` that
//!   consumes the value;
//! - `declaration`: the `file` holding the document's declaration, and the
//!   `field` that sets the value;
//! - `built-in`: nothing else.

mod check;
mod common;
mod diff;

#[cfg(test)]
mod tests;

pub use check::{
	CheckReport, ContextReport, DeadlineReport, DocumentReport, ErrorCounts,
	GatesReport, JudgementReport, OutcomeCounts, RecordReport, RuleReport,
	SeriesRef, SeriesReport, SettingReport, SettingsReport, SourceReport,
	SubjectReport, SummaryReport, UnclaimedReport, WindowReport,
};
pub use common::{
	ClockReport, DiagnosticReport, LocationReport, OriginReport, RevisionReport,
	directory, scope,
};
pub use diff::{
	BaselineReport, CurrentReport, DiffEntryReport, DiffReport, DiffSourceReport,
};

use serde::Serialize;

/// The version of every JSON format this module defines.
pub const SCHEMA: u32 = 1;

/// Serializes a report as pretty-printed JSON with sorted keys, without a
/// trailing newline.
///
/// # Errors
///
/// Fails only when `report` cannot be represented as JSON, which the report
/// types rule out: every key is a string and every number is finite.
pub fn to_json<T: Serialize>(report: &T) -> Result<String, serde_json::Error> {
	// `Value` keeps its object keys in a sorted map, which sorts the keys at
	// every level whatever order the types declare their fields in.
	let value = serde_json::to_value(report)?;
	serde_json::to_string_pretty(&value)
}
