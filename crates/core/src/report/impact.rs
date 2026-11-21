//! The JSON output of `impact --format json`.
//!
//! This shape follows the conventions of the check report: kebab-case keys,
//! sorted at every level, and a `schema`. Each document is written relative
//! to its own project, which its `project` names. Projects, the top-level
//! `project` and each entry of `not-searched`, are repository paths, with
//! `.` for the repository, as directories are throughout the reports; the
//! inputs' paths are relative to the project the query ran in.
//!
//! ```json
//! {
//!   "diagnostics": [],
//!   "documents": [
//!     {
//!       "declared-in": "docs/source-sentry.lock",
//!       "depth": 1,
//!       "document": "docs/guide.md",
//!       "field": "documents[\"guide.md\"]",
//!       "mode": "lockfile",
//!       "owner": null,
//!       "project": ".",
//!       "reached": {
//!         "kind": "direct",
//!         "matches": [{"path": "src/auth.rs", "source": "/src/*.rs"}]
//!       },
//!       "sealed": false,
//!       "settings": {"cadence": {…}, "classes": {…}, "grace": {…}, "owners": {…}}
//!     },
//!     {
//!       "document": "docs/overview.md",
//!       "depth": 2,
//!       "reached": {"document": "docs/guide.md", "kind": "via"},
//!       …
//!     }
//!   ],
//!   "inputs": [{"matched": true, "path": "src/auth.rs", "searched": true}],
//!   "not-searched": ["tools"],
//!   "project": ".",
//!   "revision": {"head": "sha1:4f3c…", "kind": "working-tree"},
//!   "schema": 1
//! }
//! ```

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use super::SCHEMA;
use super::check::{SettingsReport, mode_name};
use super::common::{DiagnosticReport, RevisionReport, directory};
use super::projects::to_project_value;
use crate::claims::{Diagnostic, Mode};
use crate::impact::{Impact, Impacted, Reached};
use crate::model::RepoPath;
use crate::ports::Revision;

/// The report of one impact query.
///
/// Build it with [`ImpactReport::new`] and serialize it with
/// [`ImpactReport::to_json`], which writes each document relative to its own
/// project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImpactReport {
	/// The errors that may hide a document, so that the answer may be
	/// incomplete.
	pub diagnostics: Vec<DiagnosticReport>,
	/// Every document reached, live documents first by depth, then sealed
	/// records.
	#[serde(skip)]
	pub documents: Vec<ImpactDocumentReport>,
	/// Each input, sorted.
	pub inputs: Vec<ImpactInputReport>,
	/// The projects that were not searched, whose documents may still cite
	/// the inputs: every project outside the one the query ran in, and each
	/// project inside it whose configs could not be loaded. Each is a
	/// repository path; `.` is the repository.
	pub not_searched: Vec<String>,
	/// The project the query ran in; `.` is the repository.
	pub project: String,
	/// The snapshot whose declarations were read.
	pub revision: RevisionReport,
	/// The format's version.
	pub schema: u32,
	/// The project the query ran in.
	#[serde(skip)]
	start: RepoPath,
}

/// One document reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImpactDocumentReport {
	/// The file that holds the declaration.
	pub declared_in: String,
	/// How many steps from an input: 1 when a source names an input.
	pub depth: usize,
	/// The document: a path or a remote identity.
	pub document: String,
	/// The declaration's field in [`ImpactDocumentReport::declared_in`].
	pub field: String,
	/// How the document is tracked: `on-file`, `lockfile` or `remote`.
	pub mode: &'static str,
	/// A remote document's owner, or `null`.
	pub owner: Option<String>,
	/// The document's project; `.` is the repository.
	pub project: String,
	/// How it was reached.
	pub reached: ReachedReport,
	/// Whether it is a sealed record, which never changes, so that nothing
	/// affects it.
	pub sealed: bool,
	/// The effective settings, each with its origin.
	pub settings: SettingsReport,
	/// The document's project, which its paths are relative to.
	#[serde(skip)]
	root: RepoPath,
}

/// How a document was reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
	tag = "kind",
	rename_all = "kebab-case",
	rename_all_fields = "kebab-case"
)]
pub enum ReachedReport {
	/// A source names an input.
	Direct {
		/// Every input and declared source that match, sorted.
		matches: Vec<MatchReport>,
	},
	/// A source names a document reached one step earlier.
	Via {
		/// That document.
		document: String,
	},
}

/// An input that a declared source names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct MatchReport {
	/// The input.
	pub path: String,
	/// The source as declared: a path or a pattern, spelled as written.
	pub source: String,
}

/// One input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImpactInputReport {
	/// Whether a source of a searched document names it.
	pub matched: bool,
	/// The file.
	pub path: String,
	/// Whether its own project was searched. When it was not, a document of
	/// that project may still cite it.
	pub searched: bool,
}

impl ImpactReport {
	/// The report of `impact`, which searched `start` and the projects nested
	/// in it, among the project roots `roots`, except those of `unloaded`,
	/// whose configs could not be loaded, in the snapshot at `revision`.
	/// `diagnostics` are the errors that may hide a document.
	#[must_use]
	pub fn new(
		impact: &Impact,
		start: &RepoPath,
		roots: &BTreeSet<RepoPath>,
		unloaded: &BTreeSet<RepoPath>,
		revision: &Revision,
		diagnostics: &[Diagnostic],
	) -> Self {
		let searched = |project: &RepoPath| {
			project.starts_with(start) && !unloaded.contains(project)
		};
		Self {
			diagnostics: diagnostics.iter().map(DiagnosticReport::from).collect(),
			documents: impact
				.documents
				.iter()
				.map(ImpactDocumentReport::from)
				.collect(),
			inputs: impact
				.inputs
				.iter()
				.map(|input| ImpactInputReport {
					matched: input.matched,
					path: input.path.to_string(),
					searched: searched(&crate::config::project_of(roots, &input.path)),
				})
				.collect(),
			not_searched: roots
				.iter()
				.filter(|root| !searched(root))
				.map(directory)
				.collect(),
			project: directory(start),
			revision: RevisionReport::from(revision),
			schema: SCHEMA,
			start: start.clone(),
		}
	}

	/// Serializes the report as pretty-printed JSON with sorted keys, without
	/// a trailing newline: each document relative to its own project, the
	/// inputs relative to the project the query ran in, and projects as
	/// repository paths.
	///
	/// # Errors
	///
	/// Fails only when the report cannot be represented as JSON, which the
	/// report types rule out.
	pub fn to_json(
		&self,
		roots: &BTreeSet<RepoPath>,
	) -> Result<String, serde_json::Error> {
		let mut value = to_project_value(self, &self.start, roots)?;
		let documents = self
			.documents
			.iter()
			.map(|document| to_project_value(document, &document.root, roots))
			.collect::<Result<Vec<_>, _>>()?;
		if let Value::Object(fields) = &mut value {
			fields.insert("documents".to_owned(), Value::Array(documents));
		}
		serde_json::to_string_pretty(&value)
	}
}

impl From<&Impacted> for ImpactDocumentReport {
	fn from(impacted: &Impacted) -> Self {
		Self {
			declared_in: impacted.declared_in.to_string(),
			depth: impacted.depth,
			document: impacted.document.to_string(),
			field: impacted.field.clone(),
			mode: mode_name(&impacted.mode),
			owner: match &impacted.mode {
				Mode::RemoteLockfile { owner } => Some(owner.clone()),
				Mode::OnFile | Mode::LocalLockfile => None,
			},
			project: directory(&impacted.project),
			reached: match &impacted.reached {
				Reached::Direct(matches) => ReachedReport::Direct {
					matches: matches
						.iter()
						.map(|found| MatchReport {
							path: found.input.to_string(),
							source: found.source.clone(),
						})
						.collect(),
				},
				Reached::Via(document) => ReachedReport::Via {
					document: document.to_string(),
				},
			},
			sealed: impacted.sealed,
			settings: SettingsReport::from(&impacted.settings),
			root: impacted.project.clone(),
		}
	}
}
