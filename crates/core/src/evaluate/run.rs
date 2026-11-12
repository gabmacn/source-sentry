//! The evaluation behind `check`: from an inventory to judged subjects and a
//! summary.
//!
//! [`check`] runs these steps:
//!
//! 1. **Policy.** It selects the context and validates every declared one,
//!    and validates the time zone. Every problem is a configuration error.
//! 2. **Hashing.** It hashes every live source and every sealed record in one
//!    batch. A path that cannot be canonicalized is left out of the facts of
//!    the documents that name it, and its diagnostic decides the exit code.
//! 3. **Planning.** For each claim it resolves the effective settings and
//!    schedule, checks a sealed record against its seal, and asks for the
//!    clock of every drifted source, every new obligation and every removed
//!    pattern member; for each series, it checks a `living` head and asks
//!    when each member completed.
//! 4. **Clocks.** It answers every drift question with one first-parent walk
//!    and every other question with another.
//! 5. **Facts.** It gives each claim its facts and next deadline, propagates
//!    `upstream-stale`, marks `awaiting-seal`, evaluates series cadences and
//!    lists unclaimed files.
//! 6. **Judgement.** It judges every subject under the context, and records
//!    every outcome and every fatal diagnostic in the summary.

mod clocks;
mod documents;
mod hashing;
mod history;
mod model;
mod projects;
pub use projects::check;
mod scopes;
mod series;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

pub use self::model::{
	AppliedContext, AppliedRule, CompletionState, DocumentState, DocumentSubject,
	Evaluation, Gates, LiveDocument, MemberCompletion, RecordCheck, RecordState,
	RuleOrigin, SeriesCadence, SeriesKey, SeriesSubject, SourceReport,
	SourceState, StartedClock, Subject, SubjectKind, Timing,
};
use self::{
	clocks::Requests,
	documents::{Finisher, Planner, propagate},
	scopes::Scopes,
};
use crate::claims::{
	Diagnostic, DiagnosticKind, Inventory, Location, Unclaimed,
};
use crate::config::{
	ConfigError, ConfigErrorKind, ConfigSet, Origin, Resolved, SUPPORTED_VERSIONS,
};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{DocumentId, Fact, RepoPath, Rule};
use crate::policy::{Summary, judge};
use crate::ports::{BackendError, History, Revision, Snapshot};

/// Evaluates `snapshot` at the instant `at` under the context named
/// `context`, or the default context when it is `None`.
///
/// # Parameters
///
/// - `snapshot`: the working tree or the commit being checked.
/// - `history`: the repository's history, read back from the snapshot's
///   revision for clocks and completions.
/// - `configs` and `inventory`: the configs loaded from `snapshot`, and its
///   discovery. Discovery's diagnostics are carried into the result, so they
///   are reported once, here.
/// - `at`: the evaluation instant, which every deadline and cadence window
///   is measured against.
/// - `context`: the context the caller selected.
///
/// Every claim is evaluated, even one in a double claim: each becomes a
/// subject of its own, and the diagnostics decide the exit code. Documents
/// whose declarations failed to parse have no claim, so they get no
/// outcome, although discovery may list them as unclaimed.
///
/// An evaluation that meets a configuration, integrity or operational error
/// still completes as far as it can, and records the error in its diagnostics
/// and summary: no context can downgrade one.
///
/// # Errors
///
/// Fails only when the backend itself fails, including when a required
/// object is missing from the object database. That is an operational error
/// (exit 4).
fn check_local<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	configs: &ConfigSet,
	inventory: &Inventory,
	at: DateTime<Utc>,
	context: Option<&str>,
	apply_policy: bool,
) -> Result<Evaluation, BackendError> {
	let revision = snapshot.revision();
	let mut diagnostics = inventory.diagnostics.clone();
	let applied = apply_policy
		.then(|| scopes::context(configs, context, &mut diagnostics))
		.flatten();
	let zone = scopes::time_zone(configs, &mut diagnostics);
	let hashes = hashing::hash(snapshot, inventory, &mut diagnostics)?;

	let mut scopes = Scopes::new(
		configs,
		applied
			.as_ref()
			.map(|context| context.selected.name.clone()),
	);
	let mut requests = Requests::default();
	let documents: Vec<_> = {
		let mut planner = Planner {
			inventory,
			hashes: &hashes,
			scopes: &mut scopes,
			requests: &mut requests,
			diagnostics: &mut diagnostics,
		};
		inventory
			.claims
			.iter()
			.map(|claim| planner.document(claim))
			.collect()
	};
	let series: Vec<_> = inventory
		.series
		.iter()
		.map(|members| {
			series::plan(
				members,
				inventory,
				&mut scopes,
				&mut requests,
				&mut diagnostics,
			)
		})
		.collect();

	let answers = requests.answer(history, revision, at)?;

	let mut finisher = Finisher {
		answers: &answers,
		zone: zone.as_ref(),
		at,
		diagnostics: &mut diagnostics,
	};
	let mut subjects: Vec<Subject> = documents
		.into_iter()
		.map(|plan| finisher.document(plan))
		.collect();
	propagate(&mut subjects, inventory);
	for plan in &series {
		let (key, members) = series::awaiting(plan);
		mark_awaiting_seal(&mut subjects, &key, members);
	}
	let series: Vec<Subject> = series
		.into_iter()
		.map(|plan| {
			series::finish(plan, &answers, zone.as_ref(), at, &mut diagnostics)
		})
		.collect();
	subjects.extend(
		inventory
			.unclaimed
			.iter()
			.map(|file| unclaimed(file, &mut scopes)),
	);
	subjects.extend(series);

	diagnostics.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
	// Different clocks can expose the same cadence's lack of a due date.
	// Keep one error at that written cadence, even when the dates differ.
	diagnostics.dedup_by(|left, right| {
		left == right
			|| (left.kind == DiagnosticKind::InvalidSchedule
				&& right.kind == DiagnosticKind::InvalidSchedule
				&& left.location == right.location)
	});
	let summary = judge_all(&mut subjects, applied.as_ref(), &diagnostics);
	Ok(Evaluation {
		project: configs.project().clone(),
		projects: configs.projects().clone(),
		revision: revision.clone(),
		at,
		version: configs.version().clone(),
		time_zone: configs.time_zone().clone(),
		context: applied,
		subjects,
		diagnostics,
		summary,
	})
}

/// The evaluation of a snapshot whose configs cannot be loaded: nothing is
/// judged, and each config error is an `invalid-config` diagnostic, except
/// invalid cadences, which keep `invalid-schedule`. A report can still be
/// written and the exit code is 3. The version and time zone are the built-in
/// ones, because no config supplied others.
#[must_use]
pub fn unloadable(
	revision: &Revision,
	at: DateTime<Utc>,
	errors: &[ConfigError],
) -> Evaluation {
	let diagnostics: Vec<Diagnostic> = errors
		.iter()
		.map(|error| {
			let location = Location {
				file: error.file.clone(),
				line: error.position.map(|position| position.line),
				column: error.position.map(|position| position.column),
				field: error.setting.clone(),
			};
			let kind = match error.kind {
				ConfigErrorKind::InvalidSchedule { .. } => {
					DiagnosticKind::InvalidSchedule
				}
				_ => DiagnosticKind::InvalidConfig,
			};
			Diagnostic::new(kind, location, error.to_string())
		})
		.collect();
	let summary = judge_all(&mut [], None, &diagnostics);
	let built_in = Origin::built_in;
	Evaluation {
		project: RepoPath::ROOT,
		projects: BTreeSet::from([RepoPath::ROOT]),
		revision: revision.clone(),
		at,
		version: Resolved {
			value: SUPPORTED_VERSIONS[SUPPORTED_VERSIONS.len() - 1],
			origin: built_in(),
		},
		time_zone: Resolved {
			value: "UTC".to_owned(),
			origin: built_in(),
		},
		context: None,
		subjects: Vec::new(),
		diagnostics,
		summary,
	}
}

/// The subject of an unclaimed file: the fact `unclaimed`, judged with the
/// defaults and rules of its directory.
fn unclaimed(file: &Unclaimed, scopes: &mut Scopes<'_>) -> Subject {
	let (defaults, rules) = scopes.of_file(&file.path);
	Subject {
		kind: SubjectKind::Unclaimed(file.clone()),
		settings: EffectiveSettings::new(&defaults, None),
		facts: BTreeSet::from([Fact::Unclaimed]),
		deadline: None,
		scoped_rules: rules,
		judgement: None,
	}
}

/// Judges every judged subject under `context`, and summarizes the outcomes
/// and every fatal diagnostic. Without a context, no
/// subject is judged and only the diagnostics are summarized.
fn judge_all(
	subjects: &mut [Subject],
	context: Option<&AppliedContext>,
	diagnostics: &[Diagnostic],
) -> Summary {
	let mut summary = Summary::new();
	if let Some(context) = context {
		for subject in subjects.iter_mut().filter(|subject| is_judged(subject)) {
			let added: Vec<Rule> = subject
				.scoped_rules
				.iter()
				.map(|rule| rule.value.clone())
				.collect();
			let judgement = judge(
				&context.selected.context,
				&added,
				&subject.settings.classes.value,
				subject.facts.iter().copied(),
			);
			summary.record_outcome(judgement.outcome);
			subject.judgement = Some(judgement);
		}
	}
	for diagnostic in diagnostics {
		if let Some(kind) = diagnostic.severity().error_kind() {
			summary.record_error(kind);
		}
	}
	summary
}

/// Whether a subject is judged: every subject except a record whose seal is
/// broken, whose file is missing or which cannot be hashed, for which an
/// error stands in.
fn is_judged(subject: &Subject) -> bool {
	!matches!(
		&subject.kind,
		SubjectKind::Document(DocumentSubject {
			state: DocumentState::Record(RecordCheck {
				state: RecordState::Broken { .. }
					| RecordState::Missing
					| RecordState::Unhashable,
				..
			}),
			..
		})
	)
}

/// Adds `awaiting-seal` to every unsealed claim of each of `members`. An
/// unsealed claim is a live document.
fn mark_awaiting_seal(
	subjects: &mut [Subject],
	key: &SeriesKey,
	members: &[RepoPath],
) {
	let members: BTreeSet<DocumentId> = members
		.iter()
		.map(|member| DocumentId::Local(member.clone()))
		.collect();
	for subject in subjects {
		if let SubjectKind::Document(document) = &mut subject.kind
			&& matches!(document.state, DocumentState::Live(_))
			&& members.contains(&document.document)
		{
			document.awaiting_seal = Some(key.clone());
			subject.facts.insert(Fact::AwaitingSeal);
		}
	}
}
