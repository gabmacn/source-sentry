//! The JSON report of `check --format json`, shaped as in
//! `structure/report.example.json`.

use serde::Serialize;

use super::SCHEMA;
use super::common::{
	ClockReport, DiagnosticReport, OriginReport, RevisionReport, directory,
	instant, scope,
};
use crate::claims::Mode;
use crate::evaluate::schedule::Deadline;
use crate::evaluate::settings::Effective;
use crate::evaluate::{
	self, DocumentState, DocumentSubject, Evaluation, RecordCheck, RecordState,
	SeriesKey, SeriesSubject, SourceState, Subject, SubjectKind,
};
use crate::model::{Cadence, ClassName, Fact, Outcome};
use crate::policy::{Basis, ErrorKind, ExitCode, Threshold};

/// The report of one evaluation under one threshold.
///
/// Build it with [`CheckReport::new`] and serialize it with
/// [`to_json`](super::to_json).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckReport {
	/// The evaluation instant.
	pub at: String,
	/// The context that judged the subjects, or `null` when none could be
	/// selected, which a configuration error explains.
	pub context: Option<ContextReport>,
	/// Every diagnostic, sorted by location.
	pub diagnostics: Vec<DiagnosticReport>,
	/// The exit code under [`CheckReport::threshold`].
	pub exit_code: u8,
	/// Both gates' answers, measured on the same evaluation.
	pub gates: GatesReport,
	/// The evaluated snapshot.
	pub revision: RevisionReport,
	/// The format's version.
	pub schema: u32,
	/// Every subject: each claim of a document, then each unclaimed file,
	/// then each series.
	pub subjects: Vec<SubjectReport>,
	/// Outcomes and errors, counted.
	pub summary: SummaryReport,
	/// The threshold the exit code was computed under: `warn` or `fail`.
	pub threshold: &'static str,
}

impl CheckReport {
	/// The report of `evaluation` under `threshold`.
	#[must_use]
	pub fn new(evaluation: &Evaluation, threshold: Threshold) -> Self {
		let gates = evaluation.gates();
		Self {
			at: instant(evaluation.at),
			context: evaluation.context.as_ref().map(|context| ContextReport {
				name: context.selected.name.clone(),
				origin: context
					.definition
					.as_ref()
					.map_or(OriginReport::BuiltIn, OriginReport::config),
			}),
			diagnostics: evaluation
				.diagnostics
				.iter()
				.map(DiagnosticReport::from)
				.collect(),
			exit_code: evaluation.exit_code(threshold).code(),
			gates: GatesReport {
				proceed: gate(gates.proceed),
				report: gate(gates.report),
			},
			revision: RevisionReport::from(&evaluation.revision),
			schema: SCHEMA,
			subjects: evaluation
				.subjects
				.iter()
				.map(|subject| SubjectReport::new(evaluation, subject))
				.collect(),
			summary: SummaryReport::new(evaluation),
			threshold: threshold.name(),
		}
	}
}

/// A gate's answer: `pass` when it exits 0, otherwise `fail`.
fn gate(code: ExitCode) -> &'static str {
	if code.passes() { "pass" } else { "fail" }
}

/// The applied context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ContextReport {
	/// Its name.
	pub name: String,
	/// Where it is defined: the repository config, or built in.
	pub origin: OriginReport,
}

/// Both gates' answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct GatesReport {
	/// The continue gate, "May the build continue?", which fails only on a
	/// failure or an error.
	#[serde(rename = "continue")]
	pub proceed: &'static str,
	/// The report gate, "Is anything wrong?".
	pub report: &'static str,
}

/// Outcomes and errors, counted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SummaryReport {
	/// Errors, by kind.
	pub errors: ErrorCounts,
	/// Judged subjects, by outcome.
	pub outcomes: OutcomeCounts,
}

impl SummaryReport {
	fn new(evaluation: &Evaluation) -> Self {
		let summary = &evaluation.summary;
		Self {
			errors: ErrorCounts {
				configuration: summary.errors(ErrorKind::Configuration),
				integrity: summary.errors(ErrorKind::Integrity),
				operational: summary.errors(ErrorKind::Operational),
			},
			outcomes: OutcomeCounts {
				fail: summary.outcomes(Outcome::Fail),
				notice: summary.outcomes(Outcome::Notice),
				pass: summary.outcomes(Outcome::Pass),
				warn: summary.outcomes(Outcome::Warn),
			},
		}
	}
}

/// Errors, by kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ErrorCounts {
	/// Configuration errors (exit 3).
	pub configuration: usize,
	/// Integrity errors (exit 3).
	pub integrity: usize,
	/// Operational errors (exit 4).
	pub operational: usize,
}

/// Judged subjects, by outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct OutcomeCounts {
	/// Failures.
	pub fail: usize,
	/// Notices.
	pub notice: usize,
	/// Passes.
	pub pass: usize,
	/// Warnings.
	pub warn: usize,
}

/// One judged thing, with its `kind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SubjectReport {
	/// One claim of a document, boxed because it is much larger than the
	/// other kinds.
	Document(Box<DocumentReport>),
	/// A file inside a root that nothing claims.
	Unclaimed(UnclaimedReport),
	/// A series.
	Series(SeriesReport),
}

impl SubjectReport {
	/// The report of `subject`, whose rules `evaluation` resolves.
	#[must_use]
	pub fn new(evaluation: &Evaluation, subject: &Subject) -> Self {
		let facts: Vec<Fact> = subject.facts.iter().copied().collect();
		let judgement = judgement(evaluation, subject);
		let outcome = subject.outcome();
		match &subject.kind {
			SubjectKind::Document(document) => Self::Document(Box::new(
				DocumentReport::new(subject, document, facts, judgement),
			)),
			SubjectKind::Unclaimed(file) => Self::Unclaimed(UnclaimedReport {
				inherit_from: file.inherit_from.as_ref().map(ToString::to_string),
				facts,
				judgement,
				outcome,
				path: file.path.to_string(),
				root: directory(&file.root),
			}),
			SubjectKind::Series(series) => {
				Self::Series(SeriesReport::new(series, facts, judgement, outcome))
			}
		}
	}
}

/// Each fact's judgement, or `None` when the subject was not judged.
fn judgement(
	evaluation: &Evaluation,
	subject: &Subject,
) -> Option<Vec<JudgementReport>> {
	let judgement = subject.judgement.as_ref()?;
	Some(
		judgement
			.facts
			.iter()
			.map(|fact| JudgementReport {
				fact: fact.fact,
				outcome: fact.outcome,
				rule: match fact.basis {
					Basis::Rule(reference) => {
						evaluation
							.rule(subject, reference)
							.map(|applied| RuleReport {
								classes: applied.rule.classes,
								facts: applied.rule.facts,
								origin: OriginReport::rule(&applied.origin),
								outcome: applied.rule.outcome,
							})
					}
					Basis::Fresh | Basis::Fallback => None,
				},
			})
			.collect(),
	)
}

/// One fact's judgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct JudgementReport {
	/// The fact.
	pub fact: Fact,
	/// Its outcome.
	pub outcome: Outcome,
	/// The rule that decided it, or `null` when the context's fallback
	/// applied, or for `fresh`, which always passes.
	pub rule: Option<RuleReport>,
}

/// A rule, with where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RuleReport {
	/// The classes it applies to; empty for any class.
	pub classes: Vec<ClassName>,
	/// The facts it applies to.
	pub facts: Vec<Fact>,
	/// Where it comes from.
	pub origin: OriginReport,
	/// Its outcome.
	pub outcome: Outcome,
}

/// One claim of a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DocumentReport {
	/// The series whose successor makes this document `awaiting-seal`.
	/// Present only then.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub awaiting_seal: Option<SeriesRef>,
	/// The next deadline, or `null` when no clock runs.
	pub deadline: Option<DeadlineReport>,
	/// The file that holds the declaration.
	pub declared_in: String,
	/// The document: a repository path or a remote identity.
	pub document: String,
	/// The facts, in order.
	pub facts: Vec<Fact>,
	/// The declaration's field in [`DocumentReport::declared_in`].
	pub field: String,
	/// Each fact's judgement, or `null` when the document was not judged.
	pub judgement: Option<Vec<JudgementReport>>,
	/// How the document is tracked: `on-file`, `lockfile` or `remote`.
	pub mode: &'static str,
	/// The outcome, or `null` when the document was not judged: when no
	/// context could be selected, or when an error stands in for a record.
	pub outcome: Option<Outcome>,
	/// A sealed record's check against its seal. Present only for records.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub record: Option<RecordReport>,
	/// The effective settings, each with its origin.
	pub settings: SettingsReport,
	/// Each source path, in declared order; a pattern contributes each
	/// current match and attested member. Empty for a record, whose sources
	/// are frozen.
	pub sources: Vec<SourceReport>,
	/// The direct upstream documents that make this one `upstream-stale`.
	/// Present only when there are any.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub stale_upstreams: Vec<String>,
	/// The clock of a document that has never been reviewed, which started
	/// at the commit that declared it. Present only for
	/// unattested documents.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub unattested: Option<ClockReport>,
}

impl DocumentReport {
	fn new(
		subject: &Subject,
		document: &DocumentSubject,
		facts: Vec<Fact>,
		judgement: Option<Vec<JudgementReport>>,
	) -> Self {
		let mut report = Self {
			awaiting_seal: document.awaiting_seal.as_ref().map(SeriesRef::from),
			deadline: subject.deadline.as_ref().map(DeadlineReport::from),
			declared_in: document.declared_in.to_string(),
			document: document.document.to_string(),
			facts,
			field: document.field.clone(),
			judgement,
			mode: match document.mode {
				Mode::OnFile => "on-file",
				Mode::LocalLockfile => "lockfile",
				Mode::RemoteLockfile { .. } => "remote",
			},
			outcome: subject.outcome(),
			record: None,
			settings: SettingsReport::new(subject),
			sources: Vec::new(),
			stale_upstreams: Vec::new(),
			unattested: None,
		};
		match &document.state {
			DocumentState::Live(live) => {
				report.sources = live.sources.iter().map(SourceReport::from).collect();
				report.stale_upstreams = live
					.stale_upstreams
					.iter()
					.map(ToString::to_string)
					.collect();
				report.unattested = live.unattested.as_ref().map(ClockReport::from);
			}
			DocumentState::Record(check) => {
				report.record = Some(RecordReport::from(check));
			}
		}
		report
	}
}

/// How a source's state is written.
fn source_state(state: SourceState) -> &'static str {
	match state {
		SourceState::Fresh => "fresh",
		SourceState::Drifted => "drifted",
		SourceState::Unattested => "unattested",
		SourceState::Added => "added",
		SourceState::Removed => "removed",
		SourceState::Missing => "missing",
		SourceState::Unhashable => "unhashable",
	}
}

/// A series, by identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SeriesRef {
	/// Its name.
	pub name: String,
	/// The scope that defines it; see [`scope`](super::scope).
	pub scope: String,
}

impl From<&SeriesKey> for SeriesRef {
	fn from(key: &SeriesKey) -> Self {
		Self {
			name: key.name.clone(),
			scope: scope(&key.scope),
		}
	}
}

/// When a subject becomes overdue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct DeadlineReport {
	/// The last local day on which it is on time, or `null` when it was due
	/// at once.
	pub due: Option<String>,
	/// The first instant at which it is overdue.
	pub overdue_from: String,
}

impl From<&Deadline> for DeadlineReport {
	fn from(deadline: &Deadline) -> Self {
		Self {
			due: deadline.due.map(|due| due.to_string()),
			overdue_from: instant(deadline.overdue_from),
		}
	}
}

/// A record checked against its seal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RecordReport {
	/// The record's canonical oid now, when its seal is broken; otherwise
	/// `null`.
	pub actual: Option<String>,
	/// The oid the seal records.
	pub seal: String,
	/// `intact`, `broken`, `missing` or `unhashable`.
	pub state: &'static str,
}

impl From<&RecordCheck> for RecordReport {
	fn from(check: &RecordCheck) -> Self {
		Self {
			actual: match &check.state {
				RecordState::Broken { actual } => Some(actual.to_string()),
				RecordState::Intact
				| RecordState::Missing
				| RecordState::Unhashable => None,
			},
			seal: check.seal.to_string(),
			state: match check.state {
				RecordState::Intact => "intact",
				RecordState::Broken { .. } => "broken",
				RecordState::Missing => "missing",
				RecordState::Unhashable => "unhashable",
			},
		}
	}
}

/// The effective settings of a subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SettingsReport {
	/// The cadence, or `null` for none.
	pub cadence: SettingReport<Option<Cadence>>,
	/// The classes.
	pub classes: SettingReport<Vec<ClassName>>,
	/// The grace period, in whole days.
	pub grace: SettingReport<u32>,
	/// The owner identifiers.
	pub owners: SettingReport<Vec<String>>,
}

impl SettingsReport {
	fn new(subject: &Subject) -> Self {
		let settings = &subject.settings;
		Self {
			cadence: SettingReport::from(&settings.cadence),
			classes: SettingReport::from(&settings.classes),
			grace: SettingReport::from(&settings.grace),
			owners: SettingReport::from(&settings.owners),
		}
	}
}

/// One setting's value, and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SettingReport<T> {
	/// Where the value comes from.
	pub origin: OriginReport,
	/// The value.
	pub value: T,
}

impl<T: Clone> From<&Effective<T>> for SettingReport<T> {
	fn from(effective: &Effective<T>) -> Self {
		Self {
			origin: OriginReport::setting(&effective.origin),
			value: effective.value.clone(),
		}
	}
}

/// One source of a live document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SourceReport {
	/// The attested oid, or `null` when it was never attested.
	pub attested: Option<String>,
	/// Its clock; see [`ClockReport`].
	pub clock: ClockReport,
	/// Its canonical oid in the snapshot, or `null` when it is missing or
	/// could not be hashed.
	pub current: Option<String>,
	/// The source path.
	pub path: String,
	/// For a pattern's member, the pattern resolved against the repository
	/// root. Present only for pattern members.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pattern: Option<String>,
	/// `fresh`, `drifted`, `unattested`, `added`, `removed`, `missing` or
	/// `unhashable`.
	pub state: &'static str,
}

impl From<&evaluate::SourceReport> for SourceReport {
	fn from(source: &evaluate::SourceReport) -> Self {
		Self {
			attested: source.attested.as_ref().map(ToString::to_string),
			clock: ClockReport::from(&source.timing),
			current: source.current.as_ref().map(ToString::to_string),
			path: source.path.to_string(),
			pattern: source.pattern.clone(),
			state: source_state(source.state),
		}
	}
}

/// An unclaimed file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct UnclaimedReport {
	/// The predecessor registration would inherit, omitted when absent.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub inherit_from: Option<String>,
	/// Its facts: `unclaimed`.
	pub facts: Vec<Fact>,
	/// Each fact's judgement, or `null` when it was not judged.
	pub judgement: Option<Vec<JudgementReport>>,
	/// The outcome, or `null` when it was not judged.
	pub outcome: Option<Outcome>,
	/// The file.
	pub path: String,
	/// The root whose scan found it.
	pub root: String,
}

/// A series.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SeriesReport {
	/// Its facts.
	pub facts: Vec<Fact>,
	/// Each fact's judgement, or `null` when it was not judged.
	pub judgement: Option<Vec<JudgementReport>>,
	/// The windows that closed without an entry, oldest first.
	pub missed: Vec<WindowReport>,
	/// Its name.
	pub name: String,
	/// The outcome, or `null` when it was not judged.
	pub outcome: Option<Outcome>,
	/// The scope that defines it; see [`scope`](super::scope).
	pub scope: String,
}

impl SeriesReport {
	fn new(
		series: &SeriesSubject,
		facts: Vec<Fact>,
		judgement: Option<Vec<JudgementReport>>,
		outcome: Option<Outcome>,
	) -> Self {
		let missed = series
			.cadence
			.as_ref()
			.and_then(|cadence| cadence.status.as_ref())
			.map(|status| {
				status
					.missed
					.iter()
					.filter_map(|&position| status.windows.get(position))
					.map(|window| WindowReport {
						after: window.after.map(|after| after.to_string()),
						due: window.due.to_string(),
						last_day: window.last_day.to_string(),
					})
					.collect()
			})
			.unwrap_or_default();
		Self {
			facts,
			judgement,
			missed,
			name: series.series.name.clone(),
			outcome,
			scope: scope(&series.series.scope),
		}
	}
}

/// A cadence window, (`after`, `last-day`], in local dates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct WindowReport {
	/// The previous due date, which the window excludes, or `null` for the
	/// first window.
	pub after: Option<String>,
	/// The window's due date.
	pub due: String,
	/// The last day the window contains: the due date plus the grace period.
	pub last_day: String,
}
