//! Shapes shared by the reports: origins, locations, revisions, diagnostics
//! and clocks.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Serialize, Serializer};

use crate::claims::{Diagnostic, Location};
use crate::config::{Origin, Scope};
use crate::evaluate::settings::SettingOrigin;
use crate::evaluate::{RuleOrigin, Timing};
use crate::model::RepoPath;
use crate::ports::Revision;

/// Where a setting, a rule or a context comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
	tag = "kind",
	rename_all = "kebab-case",
	rename_all_fields = "kebab-case"
)]
pub enum OriginReport {
	/// A config or a preset.
	Config {
		/// The file that writes the value.
		file: String,
		/// The presets through which `file` was imported: the importing
		/// config, then each preset on the way to `file`, outermost first.
		import_chain: Vec<String>,
		/// The line that writes the value, when it is known.
		line: Option<u64>,
		/// The scope that consumes the value; see [`scope`].
		scope: String,
	},
	/// The document's own declaration.
	Declaration {
		/// The field that sets the value.
		field: String,
		/// The file that holds the declaration: the document itself when it
		/// is tracked on-file, otherwise its lockfile.
		file: String,
	},
	/// Nothing sets the value, so the built-in default applies.
	BuiltIn,
}

impl OriginReport {
	/// The origin of a config value. An origin without a file is a built-in
	/// default.
	#[must_use]
	pub fn config(origin: &Origin) -> Self {
		let Some(file) = &origin.file else {
			return Self::BuiltIn;
		};
		Self::Config {
			file: file.to_string(),
			import_chain: origin
				.import_chain
				.iter()
				.map(ToString::to_string)
				.collect(),
			line: origin.position.map(|position| position.line),
			scope: scope(&origin.scope),
		}
	}

	/// The origin of an effective setting.
	#[must_use]
	pub fn setting(origin: &SettingOrigin) -> Self {
		match origin {
			SettingOrigin::BuiltIn => Self::BuiltIn,
			SettingOrigin::Config(origin) => Self::config(origin),
			SettingOrigin::Declaration { file, field } => Self::Declaration {
				field: field.clone(),
				file: file.to_string(),
			},
		}
	}

	/// The origin of a rule that a judgement names.
	///
	/// A context's own rule takes the origin of the context's definition,
	/// because the config module records origins per context rather than per
	/// rule; a built-in context's rules are built in.
	#[must_use]
	pub fn rule(origin: &RuleOrigin) -> Self {
		match origin {
			RuleOrigin::Context {
				definition: Some(definition),
				..
			}
			| RuleOrigin::Scoped(definition) => Self::config(definition),
			RuleOrigin::Context {
				definition: None, ..
			} => Self::BuiltIn,
		}
	}
}

/// How a scope is written in reports and placements:
/// `repository`, `directory:<path>` or `remote:<owner>`. The built-in
/// defaults, which are no config's scope, are written `built-in`.
#[must_use]
pub fn scope(scope: &Scope) -> String {
	match scope {
		Scope::BuiltIn => "built-in".to_owned(),
		Scope::Repository => "repository".to_owned(),
		Scope::Directory(path) => format!("directory:{}", directory(path)),
		Scope::Remote(owner) => format!("remote:{owner}"),
	}
}

/// How a directory is written: its repository path, or `.` for the
/// repository root, which [`RepoPath::new`] reads back as the root.
#[must_use]
pub fn directory(path: &RepoPath) -> String {
	if path.is_root() {
		".".to_owned()
	} else {
		path.to_string()
	}
}

/// An instant, in RFC 3339 and UTC, with fractional seconds only when they
/// are not zero.
pub(super) fn instant(at: DateTime<Utc>) -> String {
	at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// Where a diagnostic points.
///
/// The example shows `file`, `line` and `field`, so a location always has
/// those three keys, with `null` for what is unknown. A column is not
/// reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct LocationReport {
	/// The project containing the diagnostic file; `.` is the repository.
	pub project: String,
	/// The field, such as `documents["a.md"].sources[0]`, when it is known.
	pub field: Option<String>,
	/// The file.
	pub file: String,
	/// The line, counted from 1, when it is known.
	pub line: Option<u64>,
}

impl From<&Location> for LocationReport {
	fn from(location: &Location) -> Self {
		Self {
			project: ".".to_owned(),
			field: location.field.clone(),
			file: location.file.to_string(),
			line: location.line,
		}
	}
}

/// One diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DiagnosticReport {
	/// The stable code, such as `empty-pattern`.
	pub code: &'static str,
	/// Where to fix it.
	pub location: LocationReport,
	/// What is wrong and what to do about it.
	pub message: String,
	/// Other places involved, sorted.
	pub related: Vec<LocationReport>,
	/// `warning`, `configuration-error`, `integrity-error` or
	/// `operational-error`.
	pub severity: &'static str,
}

impl From<&Diagnostic> for DiagnosticReport {
	fn from(diagnostic: &Diagnostic) -> Self {
		Self {
			code: diagnostic.kind.code(),
			location: LocationReport::from(&diagnostic.location),
			message: diagnostic.message.clone(),
			related: diagnostic
				.related
				.iter()
				.map(LocationReport::from)
				.collect(),
			severity: diagnostic.severity().name(),
		}
	}
}

/// What was evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
	tag = "kind",
	rename_all = "kebab-case",
	rename_all_fields = "kebab-case"
)]
pub enum RevisionReport {
	/// The working tree.
	WorkingTree {
		/// The commit it is based on, or `null` in a repository without
		/// commits.
		head: Option<String>,
	},
	/// A committed snapshot, selected with `--rev`.
	Commit {
		/// The commit.
		commit: String,
	},
}

impl From<&Revision> for RevisionReport {
	fn from(revision: &Revision) -> Self {
		match revision {
			Revision::WorkingTree { head } => Self::WorkingTree {
				head: head.as_ref().map(ToString::to_string),
			},
			Revision::Commit(commit) => Self::Commit {
				commit: commit.to_string(),
			},
		}
	}
}

/// A clock: the commit where a source's drift began, and when.
///
/// It is written as `{"commit", "since"}` when it started, `null` when no
/// clock runs, and otherwise as a string: `not-started` when the change
/// exists only in the working tree, `unknown` when the attested oid never
/// appears in the history (`timing-unknown`), and `failed` when an
/// operational error kept it from being dated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClockReport {
	/// No clock runs.
	Idle,
	/// The change exists only in the working tree.
	NotStarted,
	/// The clock started.
	Started {
		/// The commit it started at.
		commit: String,
		/// The minimum committer date over the first-parent commits from the
		/// drift commit to the evaluated revision.
		since: String,
	},
	/// The attested oid never appears in the first-parent history.
	Unknown,
	/// The clock could not be dated.
	Failed,
}

impl From<&Timing> for ClockReport {
	fn from(timing: &Timing) -> Self {
		match timing {
			Timing::Idle => Self::Idle,
			Timing::NotStarted(_) => Self::NotStarted,
			Timing::Started(clock) => Self::Started {
				commit: clock.start.commit.to_string(),
				since: instant(clock.start.timestamp),
			},
			Timing::Unknown => Self::Unknown,
			Timing::Failed => Self::Failed,
		}
	}
}

impl Serialize for ClockReport {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		/// The object a started clock is written as.
		#[derive(Serialize)]
		struct Started<'a> {
			commit: &'a str,
			since: &'a str,
		}

		match self {
			Self::Idle => serializer.serialize_none(),
			Self::NotStarted => serializer.serialize_str("not-started"),
			Self::Started { commit, since } => {
				Started { commit, since }.serialize(serializer)
			}
			Self::Unknown => serializer.serialize_str("unknown"),
			Self::Failed => serializer.serialize_str("failed"),
		}
	}
}
