//! Phrases for people: subjects, facts, clocks, rules and origins.
//! `check` and `explain` share them.

use std::fmt::Write as _;

use chrono::{DateTime, Timelike as _, Utc};
use source_sentry_core::claims::Location;
use source_sentry_core::config::{Origin, Scope};
use source_sentry_core::evaluate::schedule::Deadline;
use source_sentry_core::evaluate::settings::SettingOrigin;
use source_sentry_core::evaluate::{
	DocumentState, Evaluation, RuleOrigin, SourceReport, SourceState, Subject,
	SubjectKind, Timing,
};
use source_sentry_core::model::{
	ClassName, CommitId, DocumentId, Fact, ObjectId, Rule,
};
use source_sentry_core::policy::Basis;
use source_sentry_core::ports::Revision;
use source_sentry_core::report;

use crate::workspace::Workspace;

/// The first seven digits of an oid, as git abbreviates it.
pub(crate) fn short(oid: &ObjectId) -> &str {
	let hex = oid.hex();
	hex.get(..7).unwrap_or(hex)
}

/// A commit, abbreviated.
pub(crate) fn short_commit(commit: &CommitId) -> &str {
	short(&commit.0)
}

/// An instant for people: `2026-11-07 18:00 UTC`, with seconds when they
/// are not zero.
pub(crate) fn instant(at: DateTime<Utc>) -> String {
	if at.second() == 0 && at.nanosecond() == 0 {
		at.format("%Y-%m-%d %H:%M UTC").to_string()
	} else {
		at.format("%Y-%m-%d %H:%M:%S UTC").to_string()
	}
}

/// The date of an instant, in UTC.
pub(crate) fn date(at: DateTime<Utc>) -> String {
	at.format("%Y-%m-%d").to_string()
}

/// What was evaluated: `working tree on 4f3c2a1`, or `commit 4f3c2a1`.
pub(crate) fn revision(revision: &Revision) -> String {
	match revision {
		Revision::WorkingTree { head: Some(head) } => {
			format!("working tree on {}", short_commit(head))
		}
		Revision::WorkingTree { head: None } => {
			"working tree, before the first commit".to_owned()
		}
		Revision::Commit(commit) => format!("commit {}", short_commit(commit)),
	}
}

/// The header of a check or an explanation: the context, the instant and
/// the revision.
pub(crate) fn header(evaluation: &Evaluation) -> String {
	let context = evaluation
		.context
		.as_ref()
		.map_or("none", |context| context.selected.name.as_str());
	format!(
		"Context {context} · {} · {}",
		instant(evaluation.at),
		revision(&evaluation.revision)
	)
}

/// A subject's name: the document, the file, or the series as
/// `<directory>: <name>`.
pub(crate) fn subject_name(workspace: &Workspace, subject: &Subject) -> String {
	match &subject.kind {
		SubjectKind::Document(document) => {
			workspace.display_document(&document.document)
		}
		SubjectKind::Unclaimed(file) => workspace.display_path(&file.path),
		SubjectKind::Series(series) => {
			let scope = match &series.series.scope {
				Scope::Directory(directory) => workspace.display_path(directory),
				other => scope(workspace, other),
			};
			format!("{scope}: {}", series.series.name)
		}
	}
}

/// The command to register an unclaimed successor and its predecessor.
pub(crate) fn inheritance_hint(
	workspace: &Workspace,
	subject: &Subject,
) -> Option<String> {
	let SubjectKind::Unclaimed(file) = &subject.kind else {
		return None;
	};
	let predecessor = file.inherit_from.as_ref()?;
	let path = workspace.document_argument(&file.path);
	let command = format!(
		"{} track {}",
		workspace.command_prefix(),
		crate::commands::shell_word(&path)
	);
	let predecessor = workspace.display_path(predecessor);
	Some(format!(
		"Run `{command}` to inherit the declaration of `{predecessor}`"
	))
}

/// A subject's facts at `at`, the instant of the check, as one line:
/// `overdue (missed review by 2026-11-03)`. `fresh` is left out, so a
/// passing subject reads `fresh`. A fact that a more urgent one implies is
/// left out too: an overdue document is also due and stale, and a document
/// with a due upstream document also has a stale one. The JSON report lists
/// every fact.
///
/// A deadline is called missed only once it has passed at `at`. A document
/// that is overdue only because a source's timing is unknown reads
/// `overdue, timing unknown`, whatever deadline its other sources set.
pub(crate) fn facts(subject: &Subject, at: DateTime<Utc>) -> String {
	let has = |fact: Fact| subject.facts.contains(&fact);
	let mut phrases = Vec::new();
	for &fact in &subject.facts {
		let phrase = match fact {
			Fact::Fresh => continue,
			Fact::Stale if has(Fact::Due) || has(Fact::Overdue) => continue,
			Fact::Due if has(Fact::Overdue) => continue,
			Fact::UpstreamStale if has(Fact::UpstreamDue) => continue,
			Fact::Stale | Fact::Due => match &subject.deadline {
				Some(deadline) => {
					format!("{fact} ({})", review_by(deadline, task(subject)))
				}
				None => fact.name().to_owned(),
			},
			Fact::Overdue => match &subject.deadline {
				Some(deadline) if is_missed(deadline, at) => match deadline.due {
					Some(due) => {
						format!("overdue (missed {} by {due})", task(subject))
					}
					None => {
						format!("overdue (since {})", date(deadline.overdue_from))
					}
				},
				Some(_) | None => "overdue".to_owned(),
			},
			Fact::Unattested => match unattested_since(subject) {
				Some(since) => format!("unattested since {since}"),
				None => "unattested".to_owned(),
			},
			Fact::Missed => missed(subject),
			Fact::TimingUnknown => "timing unknown".to_owned(),
			Fact::AwaitingSeal => "awaiting its seal".to_owned(),
			other => other.name().to_owned(),
		};
		phrases.push(phrase);
	}
	if phrases.is_empty() {
		"fresh".to_owned()
	} else {
		phrases.join(", ")
	}
}

/// Whether `deadline` has passed at `at`. A provisional deadline is never
/// missed, as it never makes a document overdue.
fn is_missed(deadline: &Deadline, at: DateTime<Utc>) -> bool {
	!deadline.provisional && deadline.is_overdue(at)
}

/// What a subject's deadline asks for: a review of a document, or the next
/// entry of a series.
fn task(subject: &Subject) -> &'static str {
	if subject.series().is_some() {
		"entry"
	} else {
		"review"
	}
}

/// When a deadline asks for `task`: `review by 2026-11-20`, `review at
/// once`, or `review by 2026-11-20 if committed today` for a change that
/// exists only in the working tree.
fn review_by(deadline: &Deadline, task: &str) -> String {
	let when = deadline.due.map_or_else(
		|| format!("{task} at once"),
		|due| format!("{task} by {due}"),
	);
	if deadline.provisional && deadline.due.is_some() {
		format!("{when} if committed today")
	} else {
		when
	}
}

/// The date an unattested document's clock started, when it did.
fn unattested_since(subject: &Subject) -> Option<String> {
	let document = subject.document()?;
	let DocumentState::Live(live) = &document.state else {
		return None;
	};
	match live.unattested.as_ref()? {
		Timing::Started(clock) => Some(date(clock.start.timestamp)),
		Timing::Idle | Timing::NotStarted(_) | Timing::Unknown | Timing::Failed => {
			None
		}
	}
}

/// A series' missed windows: `missed the window ending 2026-09-15`.
fn missed(subject: &Subject) -> String {
	let status = subject
		.series()
		.and_then(|series| series.cadence.as_ref())
		.and_then(|cadence| cadence.status.as_ref());
	let Some(status) = status else {
		return "missed".to_owned();
	};
	let windows: Vec<_> = status
		.missed
		.iter()
		.filter_map(|&position| status.windows.get(position))
		.collect();
	match windows.as_slice() {
		[] => "missed".to_owned(),
		[window] => format!("missed the window ending {}", window.last_day),
		[.., last] => format!(
			"missed {} windows, the last ending {}",
			windows.len(),
			last.last_day
		),
	}
}

/// How a source compares with its attestation, and when it changed:
/// `db/schema.sql changed in 7d2e9b0, counted from 2026-10-20`.
pub(crate) fn source_change(
	workspace: &Workspace,
	source: &SourceReport,
) -> String {
	let path = workspace.display_path(&source.path);
	let pattern = source.pattern.as_deref().unwrap_or("its pattern");
	let what = match source.state {
		SourceState::Fresh => format!("{path} is unchanged"),
		SourceState::Drifted => format!("{path} changed"),
		SourceState::Unattested => format!("{path} was never attested"),
		SourceState::Added => format!("{path} newly matches {pattern}"),
		SourceState::Removed => {
			format!("{path} no longer matches {pattern}")
		}
		SourceState::Missing => format!("{path} is missing"),
		SourceState::Unhashable => format!("{path} cannot be hashed"),
	};
	format!("{what}{}", timing_suffix(&source.timing))
}

/// When a change happened, as a suffix: ` in 7d2e9b0, counted from
/// 2026-10-20`. The date is the clock's, the earliest committer date from
/// that commit on, which is not the commit's own date when a later commit is
/// dated earlier.
pub(crate) fn timing_suffix(timing: &Timing) -> String {
	match timing {
		Timing::Idle => String::new(),
		Timing::NotStarted(_) => " in the working tree".to_owned(),
		Timing::Started(clock) => format!(
			" in {}, counted from {}",
			short_commit(&clock.start.commit),
			date(clock.start.timestamp)
		),
		Timing::Unknown => {
			", at an unknown time: the attested oid is not in the history".to_owned()
		}
		Timing::Failed => ", at a time that could not be dated".to_owned(),
	}
}

/// The stale sources of a reviewed document and its stale upstreams, one
/// line each. An unattested document's sources are all unattested, which
/// its facts already say. An upstream document that is due is named with
/// its own deadline, from `evaluation`.
pub(crate) fn changes(
	workspace: &Workspace,
	evaluation: &Evaluation,
	subject: &Subject,
) -> Vec<String> {
	let Some(document) = subject.document() else {
		return Vec::new();
	};
	let DocumentState::Live(live) = &document.state else {
		return Vec::new();
	};
	live
		.sources
		.iter()
		.filter(|source| live.unattested.is_none() && source.state.is_stale())
		.map(|source| source_change(workspace, source))
		.chain(live.stale_upstreams.iter().map(|upstream| {
			let path = workspace.display_path(upstream);
			if !live.due_upstreams.contains(upstream) {
				return format!("its source {path} is not up to date");
			}
			let due = evaluation
				.subjects_of(&DocumentId::Local(upstream.clone()))
				.find(|found| found.facts.contains(&Fact::Due));
			match due.and_then(|found| found.deadline.as_ref()) {
				Some(deadline) => format!(
					"its source {path} is due ({})",
					review_by(deadline, "review")
				),
				None if due.is_some() => format!("its source {path} is due"),
				None => format!("its source {path} waits on a document that is due"),
			}
		}))
		.collect()
}

/// The rules that decided a subject's outcome, one line each, with the
/// facts each decided: `production: privacy + stale, overdue → fail
/// (.source-sentry/source-sentry.yaml:20)`. A fallback decides without a
/// rule, so it has no line.
pub(crate) fn deciding_rules(
	workspace: &Workspace,
	evaluation: &Evaluation,
	subject: &Subject,
) -> Vec<String> {
	let Some(judgement) = &subject.judgement else {
		return Vec::new();
	};
	// Each deciding rule, as its origin and condition, with the facts it
	// decided, in the order the facts come.
	let mut lines: Vec<(String, String, Vec<Fact>)> = Vec::new();
	for fact in &judgement.facts {
		let Basis::Rule(reference) = fact.basis else {
			continue;
		};
		if fact.outcome != judgement.outcome {
			continue;
		}
		let Some(applied) = evaluation.rule(subject, reference) else {
			continue;
		};
		let origin = rule_origin(workspace, &applied.origin);
		let condition = rule_condition(&applied.rule);
		match lines.iter_mut().find(|(known, known_condition, _)| {
			*known == origin && *known_condition == condition
		}) {
			Some((_, _, facts)) => facts.push(fact.fact),
			None => lines.push((origin, condition, vec![fact.fact])),
		}
	}
	let context = evaluation
		.context
		.as_ref()
		.map_or("", |context| context.selected.name.as_str());
	lines
		.into_iter()
		.map(|(origin, condition, facts)| {
			let facts: Vec<&str> = facts.iter().map(|fact| fact.name()).collect();
			format!(
				"{context}: {condition}{} → {} ({origin})",
				facts.join(", "),
				judgement.outcome
			)
		})
		.collect()
}

/// A rule's classes, as a prefix: `privacy + `, or nothing for any class.
fn rule_condition(rule: &Rule) -> String {
	if rule.classes.is_empty() {
		String::new()
	} else {
		let classes: Vec<&str> =
			rule.classes.iter().map(ClassName::as_str).collect();
		format!("{} + ", classes.join(", "))
	}
}

/// A rule in full: `privacy + stale, overdue → fail`.
pub(crate) fn rule(rule: &Rule) -> String {
	let facts: Vec<&str> = rule.facts.iter().map(|fact| fact.name()).collect();
	format!(
		"{}{} → {}",
		rule_condition(rule),
		facts.join(", "),
		rule.outcome
	)
}

/// Where a rule comes from, briefly.
pub(crate) fn rule_origin(
	workspace: &Workspace,
	origin: &RuleOrigin,
) -> String {
	match origin {
		RuleOrigin::Context {
			definition: Some(definition),
			..
		}
		| RuleOrigin::Scoped(definition) => config_origin(workspace, definition),
		RuleOrigin::Context {
			definition: None, ..
		} => "built-in".to_owned(),
	}
}

/// Where a config value comes from: `docs/source-sentry.yaml:7`, with the
/// presets it was imported through, or `built-in`.
pub(crate) fn config_origin(workspace: &Workspace, origin: &Origin) -> String {
	let Some(file) = &origin.file else {
		return "built-in".to_owned();
	};
	let mut text = workspace.display_path(file);
	if let Some(position) = origin.position {
		let _ = write!(text, ":{}", position.line);
	}
	if !origin.import_chain.is_empty() {
		let chain: Vec<String> = origin
			.import_chain
			.iter()
			.map(|file| workspace.display_path(file))
			.collect();
		let _ = write!(text, ", imported through {}", chain.join(" → "));
	}
	text
}

/// Where an effective setting comes from, with the scope of a config.
pub(crate) fn setting_origin(
	workspace: &Workspace,
	origin: &SettingOrigin,
) -> String {
	match origin {
		SettingOrigin::BuiltIn => "built-in".to_owned(),
		SettingOrigin::Config(origin) if origin.file.is_none() => {
			"built-in".to_owned()
		}
		SettingOrigin::Config(origin) => format!(
			"{} (for {})",
			config_origin(workspace, origin),
			scope(workspace, &origin.scope)
		),
		SettingOrigin::Declaration { file, field } => {
			format!("{} `{field}`", workspace.display_path(file))
		}
	}
}

/// Where a diagnostic points: `file:line:column`, or the file and field
/// when the line is unknown.
pub(crate) fn location(workspace: &Workspace, location: &Location) -> String {
	let mut text = workspace.display_path(&location.file);
	match (location.line, location.column) {
		(Some(line), Some(column)) => {
			let _ = write!(text, ":{line}:{column}");
		}
		(Some(line), None) => {
			let _ = write!(text, ":{line}");
		}
		(None, _) => {
			if let Some(field) = &location.field {
				let _ = write!(text, " `{field}`");
			}
		}
	}
	text
}

/// A subject's deadline in full; see [`deadline`]. A series asks for its
/// next entry rather than a review.
pub(crate) fn subject_deadline(subject: &Subject) -> Option<String> {
	let deadline = subject.deadline.as_ref()?;
	let text = self::deadline(deadline);
	Some(match subject.series() {
		Some(_) => text.replacen("review", "entry", 1),
		None => text,
	})
}

/// A deadline in full: `review by 2026-11-03: due from 2026-11-03 00:00 UTC,
/// overdue from 2026-11-04 00:00 UTC`. A provisional deadline says that it
/// holds only if the change is committed now.
pub(crate) fn deadline(deadline: &Deadline) -> String {
	match (deadline.due, deadline.provisional) {
		(Some(due), false) => format!(
			"review by {due}: due from {}, overdue from {}",
			instant(deadline.due_from),
			instant(deadline.overdue_from)
		),
		(None, false) => format!(
			"review at once: due and overdue from {}",
			instant(deadline.overdue_from)
		),
		(Some(due), true) => format!(
			"review by {due} if committed now: due from {}, overdue from {}; \
			 it moves when the change is committed",
			instant(deadline.due_from),
			instant(deadline.overdue_from)
		),
		(None, true) => {
			"review at once: due now, and overdue once the change is committed"
				.to_owned()
		}
	}
}

/// "1 failing", "2 warnings": a count and its noun.
pub(crate) fn count(count: usize, singular: &str, plural: &str) -> String {
	if count == 1 {
		format!("1 {singular}")
	} else {
		format!("{count} {plural}")
	}
}

/// A scope with directory paths relative to the current directory.
pub(crate) fn scope(workspace: &Workspace, value: &Scope) -> String {
	match value {
		Scope::Directory(directory) => {
			format!("directory:{}", workspace.display_path(directory))
		}
		other => report::scope(other),
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeSet;

	use chrono::{DateTime, NaiveDate, Utc};
	use source_sentry_core::claims::Mode;
	use source_sentry_core::config::DocumentDefaults;
	use source_sentry_core::evaluate::schedule::Deadline;
	use source_sentry_core::evaluate::settings::EffectiveSettings;
	use source_sentry_core::evaluate::{
		DocumentState, DocumentSubject, LiveDocument, Subject, SubjectKind,
	};
	use source_sentry_core::model::{DocumentId, Fact, RepoPath};

	use super::facts;

	/// An instant written in RFC 3339.
	fn instant(text: &str) -> DateTime<Utc> {
		DateTime::parse_from_rfc3339(text)
			.expect("a valid instant")
			.with_timezone(&Utc)
	}

	/// A path in the repository.
	fn path(text: &str) -> RepoPath {
		RepoPath::new(text).expect("a valid path")
	}

	/// The deadline of a source whose timing is known: review by 2026-12-15.
	fn known_deadline(provisional: bool) -> Deadline {
		Deadline {
			due: Some(NaiveDate::from_ymd_opt(2026, 12, 15).expect("a valid date")),
			due_from: instant("2026-12-15T00:00:00Z"),
			overdue_from: instant("2026-12-16T00:00:00Z"),
			provisional,
		}
	}

	/// A document with a source whose timing is unknown, which makes it
	/// stale, due, overdue and timing-unknown at once, and the deadline its
	/// other sources set.
	fn timing_unknown(deadline: Option<Deadline>) -> Subject {
		Subject {
			kind: SubjectKind::Document(DocumentSubject {
				document: DocumentId::Local(path("doc.md")),
				mode: Mode::LocalLockfile,
				declared_in: path("source-sentry.lock"),
				field: "documents[\"doc.md\"]".to_owned(),
				state: DocumentState::Live(LiveDocument {
					unattested: None,
					sources: Vec::new(),
					stale_upstreams: Vec::new(),
					due_upstreams: Vec::new(),
				}),
				awaiting_seal: None,
			}),
			settings: EffectiveSettings::new(&DocumentDefaults::default(), None),
			facts: BTreeSet::from([
				Fact::Stale,
				Fact::Due,
				Fact::Overdue,
				Fact::TimingUnknown,
			]),
			deadline,
			scoped_rules: Vec::new(),
			judgement: None,
		}
	}

	/// A deadline a month ahead is not missed, even though the unknown
	/// timing makes the document overdue now.
	#[test]
	fn unknown_timing_does_not_call_a_future_deadline_missed() {
		let subject = timing_unknown(Some(known_deadline(false)));
		assert_eq!(
			facts(&subject, instant("2026-11-16T12:00:00Z")),
			"overdue, timing unknown"
		);
	}

	/// Once the deadline of a source whose timing is known has passed, the
	/// line says it was missed, as for any overdue document.
	#[test]
	fn unknown_timing_with_a_passed_deadline_names_the_missed_date() {
		let subject = timing_unknown(Some(known_deadline(false)));
		assert_eq!(
			facts(&subject, instant("2026-12-16T00:00:00Z")),
			"overdue (missed review by 2026-12-15), timing unknown"
		);
	}

	/// With no other clock, the line names no deadline.
	#[test]
	fn unknown_timing_alone_names_no_deadline() {
		let subject = timing_unknown(None);
		assert_eq!(
			facts(&subject, instant("2026-11-16T12:00:00Z")),
			"overdue, timing unknown"
		);
	}

	/// A provisional deadline never makes a document overdue, so it is
	/// never called missed, even once its date has passed.
	#[test]
	fn a_provisional_deadline_is_never_called_missed() {
		let subject = timing_unknown(Some(known_deadline(true)));
		assert_eq!(
			facts(&subject, instant("2026-12-20T00:00:00Z")),
			"overdue, timing unknown"
		);
	}
}
