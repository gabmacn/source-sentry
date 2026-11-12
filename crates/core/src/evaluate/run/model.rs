//! What an evaluation produces: subjects, their facts and judgements, and the
//! evidence behind each.
//!
//! Every type here owns its data, so the report and the CLI can keep an
//! [`Evaluation`] after the inputs are gone. None of them implements
//! `serde::Serialize`: several of the values they carry, such as
//! [`Origin`], [`Judgement`] and [`Revision`], belong to other modules and
//! are not serializable, so the report module maps them to its own JSON
//! shapes.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::claims::{Diagnostic, Mode, SeriesMember, Unclaimed};
use crate::config::{Origin, Resolved, Scope, Series};
use crate::evaluate::cadence::CadenceStatus;
use crate::evaluate::clock::ClockStart;
use crate::evaluate::schedule::Deadline;
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{
	DocumentId, Fact, Lifecycle, ObjectId, Outcome, RepoPath, Rule,
};
use crate::policy::{
	ContextOrigin, ErrorKind, ExitCode, Judgement, RuleRef, SelectedContext,
	Summary, Threshold,
};
use crate::ports::Revision;

/// The result of [`check`](super::check): every subject with its facts and
/// judgement, every diagnostic, and the summary that decides the exit code.
///
/// An evaluation that met an operational error is still returned, with the
/// error among its diagnostics: [`Evaluation::is_complete`] then reports
/// `false` and the exit code is 4, which distinguishes it from a completed
/// evaluation whose timing is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
	/// The project evaluated.
	pub project: RepoPath,
	/// Project markers from the evaluated snapshot.
	pub projects: BTreeSet<RepoPath>,
	/// The evaluated snapshot.
	pub revision: Revision,
	/// The evaluation instant, against which every deadline is measured.
	pub at: DateTime<Utc>,
	/// The schema and default-policy version, with its origin.
	pub version: Resolved<u32>,
	/// The configured time zone, as written, with its origin.
	pub time_zone: Resolved<String>,
	/// The context whose rules judged the subjects, or `None` when no context
	/// could be selected, in which case no subject is judged and a
	/// configuration error says why.
	pub context: Option<AppliedContext>,
	/// Every subject: each claim of a document in inventory order, then each
	/// unclaimed file by path, then each series by scope and name.
	pub subjects: Vec<Subject>,
	/// Every diagnostic, discovery's and evaluation's, sorted by location and
	/// without repeats.
	pub diagnostics: Vec<Diagnostic>,
	/// Each judged subject's outcome, and each fatal diagnostic by kind.
	pub summary: Summary,
}

impl Evaluation {
	/// The exit code under `threshold`. See
	/// [`Summary::exit_code`].
	#[must_use]
	pub fn exit_code(&self, threshold: Threshold) -> ExitCode {
		self.summary.exit_code(threshold)
	}

	/// The results of both gates, measured on this one evaluation.
	#[must_use]
	pub fn gates(&self) -> Gates {
		Gates {
			report: self.exit_code(Threshold::Warn),
			proceed: self.exit_code(Threshold::Fail),
		}
	}

	/// Whether the evaluation completed: no operational error was recorded.
	/// A completed evaluation may still carry `timing-unknown`.
	#[must_use]
	pub fn is_complete(&self) -> bool {
		self.summary.errors(ErrorKind::Operational) == 0
	}

	/// Every subject for `document`: one per claim, so more than one for a
	/// double claim.
	pub fn subjects_of<'a>(
		&'a self,
		document: &DocumentId,
	) -> impl Iterator<Item = &'a Subject> + use<'a> {
		let document = document.clone();
		self.subjects.iter().filter(move |subject| {
			subject
				.document()
				.is_some_and(|found| found.document == document)
		})
	}

	/// The rule that `reference`, taken from `subject`'s judgement, points
	/// at, with where it comes from. `None` when no context was
	/// applied or the reference is out of range.
	#[must_use]
	pub fn rule(
		&self,
		subject: &Subject,
		reference: RuleRef,
	) -> Option<AppliedRule> {
		match reference {
			RuleRef::Context(index) => {
				let context = self.context.as_ref()?;
				let rule = context.selected.context.rules.get(index)?;
				Some(AppliedRule {
					rule: rule.clone(),
					origin: RuleOrigin::Context {
						name: context.selected.name.clone(),
						origin: context.selected.origin,
						definition: context
							.rule_definitions
							.get(index)
							.or(context.definition.as_ref())
							.cloned(),
					},
				})
			}
			RuleRef::Scoped(index) => {
				let scoped = subject.scoped_rules.get(index)?;
				Some(AppliedRule {
					rule: scoped.value.clone(),
					origin: RuleOrigin::Scoped(scoped.origin.clone()),
				})
			}
		}
	}
}

/// The exit codes of the two gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Gates {
	/// The report gate, "Is anything wrong?", measured at
	/// [`Threshold::Warn`].
	pub report: ExitCode,
	/// The continue gate, "May the build continue?", measured at
	/// [`Threshold::Fail`].
	pub proceed: ExitCode,
}

/// The context applied to an evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedContext {
	/// The context as selected.
	pub selected: SelectedContext,
	/// Where the repository config declares it, or `None` for the built-in
	/// default context.
	pub definition: Option<Origin>,
	/// Where the repository config declares each of its rules, in order, or
	/// empty for the built-in default context.
	pub rule_definitions: Vec<Origin>,
}

/// A rule that a judgement refers to, with its origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedRule {
	/// The rule.
	pub rule: Rule,
	/// Where it comes from.
	pub origin: RuleOrigin,
}

/// Where a rule comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleOrigin {
	/// One of the selected context's own rules.
	Context {
		/// The context's name.
		name: String,
		/// Whether it is built in or declared.
		origin: ContextOrigin,
		/// Where the repository config declares the rule, or `None` when the
		/// context is built in.
		definition: Option<Origin>,
	},
	/// A rule that a directory config, a preset or the repository config adds
	/// to the context for the subject's scope.
	Scoped(Origin),
}

/// One judged thing: a claim of a document, an unclaimed file, or a series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
	/// What is judged, and the evidence specific to it.
	pub kind: SubjectKind,
	/// The effective classes, owners, grace and cadence, each with its origin.
	/// An unclaimed file and a series take the defaults of their directory or
	/// scope.
	pub settings: EffectiveSettings,
	/// The facts.
	pub facts: BTreeSet<Fact>,
	/// The next deadline: for a document, the earliest outstanding one of its
	/// clocks; for a series, the end of its first uncredited window
	/// that is not missed. It lies in the past when the subject is
	/// overdue. `None` when no clock runs.
	pub deadline: Option<Deadline>,
	/// The rules that configs add to the applied context for this subject's
	/// scope, in resolution order. [`RuleRef::Scoped`] indexes this list.
	pub scoped_rules: Vec<Resolved<Rule>>,
	/// The judgement under the applied context, or `None` when no context was
	/// applied, or when an error stands in for the outcome: a record whose
	/// seal is broken, whose file is missing, or which cannot be hashed.
	pub judgement: Option<Judgement>,
}

impl Subject {
	/// The document, when the subject is one.
	#[must_use]
	pub fn document(&self) -> Option<&DocumentSubject> {
		match &self.kind {
			SubjectKind::Document(document) => Some(document),
			SubjectKind::Unclaimed(_) | SubjectKind::Series(_) => None,
		}
	}

	/// The series, when the subject is one.
	#[must_use]
	pub fn series(&self) -> Option<&SeriesSubject> {
		match &self.kind {
			SubjectKind::Series(series) => Some(series),
			SubjectKind::Document(_) | SubjectKind::Unclaimed(_) => None,
		}
	}

	/// The outcome, when the subject was judged.
	#[must_use]
	pub fn outcome(&self) -> Option<Outcome> {
		self.judgement.as_ref().map(|judgement| judgement.outcome)
	}
}

/// What a subject is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubjectKind {
	/// One claim of a document.
	Document(DocumentSubject),
	/// An eligible file inside a root that nothing claims. Its only fact is
	/// `unclaimed`.
	Unclaimed(Unclaimed),
	/// A series, whose facts come from its cadence.
	Series(SeriesSubject),
}

/// One claim of a document, evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSubject {
	/// The document.
	pub document: DocumentId,
	/// How the claim tracks it.
	pub mode: Mode,
	/// The file that holds the declaration.
	pub declared_in: RepoPath,
	/// The declaration's field in that file.
	pub field: String,
	/// What was found.
	pub state: DocumentState,
	/// The series that makes this document carry `awaiting-seal`, if any.
	pub awaiting_seal: Option<SeriesKey>,
}

/// How a document was evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentState {
	/// A live obligation: its sources are compared with their attestations.
	Live(LiveDocument),
	/// A sealed record: the document is compared with its seal, and its
	/// sources are frozen.
	Record(RecordCheck),
}

/// The evidence for a live document's facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveDocument {
	/// When the document has never been reviewed, the clock of its
	/// declaration, which started at the commit that introduced it. `None`
	/// when the document has been reviewed.
	pub unattested: Option<Timing>,
	/// Each source path, in declared order; a pattern contributes each of
	/// its current matches and attested members, sorted by path.
	pub sources: Vec<SourceReport>,
	/// The direct upstream documents that carry `stale`, `overdue`,
	/// `unattested`, `timing-unknown` or `upstream-stale`, which make this
	/// document `upstream-stale`.
	pub stale_upstreams: Vec<RepoPath>,
}

/// One source of a live document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceReport {
	/// The source's index in the declaration's `sources`.
	pub index: usize,
	/// For a pattern member, the pattern resolved against the repository
	/// root, such as `/docs/*.md`; `None` for a path source.
	pub pattern: Option<String>,
	/// The source path.
	pub path: RepoPath,
	/// The attested oid, if the source was ever attested.
	pub attested: Option<ObjectId>,
	/// The canonical oid in the evaluated snapshot, when the source exists and
	/// could be hashed.
	pub current: Option<ObjectId>,
	/// How the source compares with its attestation.
	pub state: SourceState,
	/// Its clock.
	pub timing: Timing,
}

/// How one source compares with its attestation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceState {
	/// Its canonical oid equals the attested oid.
	Fresh,
	/// Its canonical oid differs from the attested oid.
	Drifted,
	/// A path source that has never been attested.
	Unattested,
	/// A current match of a pattern that the attestation does not list: a
	/// new, unattested source.
	Added,
	/// An attested member that the pattern no longer matches, because the
	/// snapshot lacks it: a change. Its clock is searched as drift
	/// towards the absent path, so a member that matched its attestation until
	/// it was deleted starts at the deleting commit, and one that drifted
	/// earlier keeps that earlier clock.
	Removed,
	/// An explicit path source that the snapshot lacks. Discovery reports
	/// it, and it contributes no fact.
	Missing,
	/// Its canonical oid could not be computed. A diagnostic says why, and it
	/// contributes no fact.
	Unhashable,
}

impl SourceState {
	/// Whether the source makes a reviewed document `stale`.
	#[must_use]
	pub fn is_stale(self) -> bool {
		matches!(
			self,
			Self::Drifted | Self::Unattested | Self::Added | Self::Removed
		)
	}
}

/// A clock, as the first-parent history dates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Timing {
	/// No clock runs for this source: it is fresh, it contributes no fact, or
	/// the document is unattested and its declaration's clock applies.
	Idle,
	/// The obligation exists only in the working tree, so its clock has not
	/// started and it has no deadline.
	NotStarted,
	/// The clock started.
	Started(StartedClock),
	/// The attested oid never appears in the first-parent history, so the
	/// source is `timing-unknown`, `stale` and `overdue` at once, with no
	/// grace or cadence deferral.
	Unknown,
	/// The clock could not be dated: the history is shallow or the
	/// timestamp is too far in the future. An operational error says which.
	Failed,
}

/// A clock that started, with the deadline it gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedClock {
	/// The commit it started at, and the timestamp that dates it.
	pub start: ClockStart,
	/// When it becomes overdue under the subject's schedule, or `None`
	/// when the schedule or the time zone is invalid, which a configuration
	/// error reports.
	pub deadline: Option<Deadline>,
}

/// A sealed record, checked against its seal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordCheck {
	/// The canonical oid the seal records.
	pub seal: ObjectId,
	/// What the check found.
	pub state: RecordState,
}

/// What checking a record against its seal found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordState {
	/// The record's canonical oid equals its seal, so it is `fresh`.
	Intact,
	/// The record changed: an integrity error.
	Broken {
		/// Its canonical oid now.
		actual: ObjectId,
	},
	/// The record's file is missing, which discovery reports.
	Missing,
	/// The record could not be hashed, which a diagnostic reports.
	Unhashable,
}

/// A series' identity: its scope plus its name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeriesKey {
	/// The scope of the config that defines or imports it.
	pub scope: Scope,
	/// Its name.
	pub name: String,
	/// Its lifecycle.
	pub lifecycle: Lifecycle,
}

/// A series, evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesSubject {
	/// The series, as configured.
	pub series: Series,
	/// Its members, oldest first, as discovery found them.
	pub members: Vec<SeriesMember>,
	/// The living entry of a `living` series: its newest member. `None` for
	/// a `records-only` series or a series without members.
	pub head: Option<RepoPath>,
	/// The cadence's evaluation, when the effective schedule has a cadence.
	pub cadence: Option<SeriesCadence>,
}

/// A series' cadence, evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesCadence {
	/// Every incarnation that has completed or is about to, by path: a sealed
	/// record in a `records-only` series, or an entry with `completed` in a
	/// `living` one. A reused path may have several completions.
	pub completions: Vec<MemberCompletion>,
	/// The windows, credits, and `overdue` and `missed`, or `None` when the
	/// cadence, the grace period or the time zone is invalid, or a date fell
	/// outside the calendar, which a diagnostic reports.
	pub status: Option<CadenceStatus<RepoPath>>,
}

/// When one series member completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCompletion {
	/// The member.
	pub member: RepoPath,
	/// Whether and when it completed.
	pub state: CompletionState,
}

/// Whether a member's completion counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionState {
	/// It completed at the commit that introduced its seal or `completed`,
	/// dated like a clock start.
	Completed(ClockStart),
	/// Its seal or `completed` exists only in the working tree, so it does
	/// not count yet.
	Uncommitted,
	/// Its completion could not be dated, which an operational error
	/// reports.
	Failed,
}
