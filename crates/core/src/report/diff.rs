//! The JSON output of `diff --format json`.
//!
//! This shape follows the conventions of the check report: kebab-case keys,
//! sorted at every level, and a `schema`.
//!
//! ```json
//! {
//!   "attest": ["source-sentry", "attest", "/docs/privacy.md", "--expect", "/db/schema.sql@sha1:9c11…"],
//!   "document": "docs/privacy.md",
//!   "expectations": ["db/schema.sql@sha1:9c11…"],
//!   "revision": {"head": "sha1:4f3c…", "kind": "working-tree"},
//!   "schema": 1,
//!   "sources": [
//!     {
//!       "declared": "/db/schema.sql",
//!       "entries": [
//!         {
//!           "baseline": {"oid": "sha1:58e6…", "state": "available"},
//!           "changed": true,
//!           "current": {"oid": "sha1:9c11…", "state": "present"},
//!           "diff": "@@ -1 +1 @@\n-old\n+new\n",
//!           "path": "db/schema.sql"
//!         }
//!       ],
//!       "pattern": false,
//!       "reviewed": {"at": "2026-11-07T14:32:05Z", "by": null}
//!     }
//!   ]
//! }
//! ```

use serde::Serialize;

use super::SCHEMA;
use super::common::RevisionReport;
use crate::mutate::{Baseline, Current, DocumentDiff, EntryDiff};
use crate::ports::Revision;

/// A document's selected sources, compared with their attestations, and the
/// `attest` command that binds what was shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DiffReport {
	/// The document project; `.` is the repository root.
	pub project: String,
	/// The arguments of the `attest` command that binds exactly what was
	/// shown, expectations included, starting with the program's name.
	pub attest: Vec<String>,
	/// The document.
	pub document: String,
	/// The expectations to pass to `attest`, written `path@oid` or
	/// `path@absent`, sorted by path.
	pub expectations: Vec<String>,
	/// The snapshot compared, which is the working tree.
	pub revision: RevisionReport,
	/// The format's version.
	pub schema: u32,
	/// Each selected source, in declared order.
	pub sources: Vec<DiffSourceReport>,
}

impl DiffReport {
	/// The report of `diff`, taken from a snapshot at `revision`.
	///
	/// # Parameters
	///
	/// - `attest`: the arguments of the `attest` command that the caller
	///   prints, starting with the program's name.
	/// - `unified`: renders the hunks of a unified diff from an old text to a
	///   new one, without file headers. It is called only for a file that
	///   changed and whose two sides are text; a side that does not exist
	///   is empty.
	#[must_use]
	pub fn new(
		diff: &DocumentDiff,
		revision: &Revision,
		attest: Vec<String>,
		unified: impl Fn(&str, &str) -> String,
	) -> Self {
		Self {
			project: ".".to_owned(),
			attest,
			document: diff.document.to_string(),
			expectations: diff.expectations.iter().map(ToString::to_string).collect(),
			revision: RevisionReport::from(revision),
			schema: SCHEMA,
			sources: diff
				.sources
				.iter()
				.map(|source| DiffSourceReport {
					declared: source.declared.clone(),
					entries: source
						.entries
						.iter()
						.map(|entry| DiffEntryReport::new(entry, &unified))
						.collect(),
					pattern: source.pattern,
					reviewed: source.reviewed.clone(),
				})
				.collect(),
		}
	}
}

/// One selected source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DiffSourceReport {
	/// The path or pattern, as declared.
	pub declared: String,
	/// The path of a path source, or every current and former member of a
	/// pattern, sorted by path.
	pub entries: Vec<DiffEntryReport>,
	/// Whether it is a pattern.
	pub pattern: bool,
	/// The source's own review, when recorded. Member baselines remain separate.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub reviewed: Option<crate::model::Review>,
}

/// One file, as attested and as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DiffEntryReport {
	/// What was attested.
	pub baseline: BaselineReport,
	/// Whether the file differs from what was attested: changed, added,
	/// removed, or never attested.
	pub changed: bool,
	/// What the snapshot holds.
	pub current: CurrentReport,
	/// The hunks of a unified diff from the baseline to the current file, or
	/// `null` when the file is unchanged, or when either side is not UTF-8
	/// text. An unavailable baseline diffs as empty, so that the whole
	/// current file is shown, as `baseline.state` says.
	pub diff: Option<String>,
	/// The file.
	pub path: String,
}

impl DiffEntryReport {
	fn new(entry: &EntryDiff, unified: &impl Fn(&str, &str) -> String) -> Self {
		let changed = entry.is_changed();
		let old = match &entry.baseline {
			// An unavailable baseline shows the whole file, like a new one.
			Baseline::Unattested | Baseline::Unavailable { .. } => Some(""),
			Baseline::Available { content, .. } => std::str::from_utf8(content).ok(),
		};
		let new = match &entry.current {
			Current::Missing => Some(""),
			Current::Present { content, .. } => content
				.as_deref()
				.and_then(|content| std::str::from_utf8(content).ok()),
		};
		let diff = match (changed, old, new) {
			(true, Some(old), Some(new)) => Some(unified(old, new)),
			_ => None,
		};
		Self {
			baseline: BaselineReport::from(&entry.baseline),
			changed,
			current: CurrentReport::from(&entry.current),
			diff,
			path: entry.path.to_string(),
		}
	}
}

/// What was attested for one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BaselineReport {
	/// The attested oid, or `null` when the file was never attested.
	pub oid: Option<String>,
	/// `unattested`; `unavailable` when the attested blob is not in the
	/// object database; or `available`.
	pub state: &'static str,
}

impl From<&Baseline> for BaselineReport {
	fn from(baseline: &Baseline) -> Self {
		match baseline {
			Baseline::Unattested => Self {
				oid: None,
				state: "unattested",
			},
			Baseline::Unavailable { oid } => Self {
				oid: Some(oid.to_string()),
				state: "unavailable",
			},
			Baseline::Available { oid, .. } => Self {
				oid: Some(oid.to_string()),
				state: "available",
			},
		}
	}
}

/// What the snapshot holds for one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CurrentReport {
	/// Its canonical oid, or `null` when it has left its pattern.
	pub oid: Option<String>,
	/// `present`, or `missing` when it has left its pattern.
	pub state: &'static str,
}

impl From<&Current> for CurrentReport {
	fn from(current: &Current) -> Self {
		match current {
			Current::Missing => Self {
				oid: None,
				state: "missing",
			},
			Current::Present { oid, .. } => Self {
				oid: Some(oid.to_string()),
				state: "present",
			},
		}
	}
}
