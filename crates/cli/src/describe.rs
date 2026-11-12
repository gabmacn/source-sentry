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
use source_sentry_core::model::{ClassName, CommitId, Fact, ObjectId, Rule};
use source_sentry_core::policy::Basis;
use source_sentry_core::ports::Revision;
use source_sentry_core::report;

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
pub(crate) fn subject_name(subject: &Subject) -> String {
	match &subject.kind {
		SubjectKind::Document(document) => document.document.to_string(),
		SubjectKind::Unclaimed(file) => file.path.to_string(),
		SubjectKind::Series(series) => {
			let scope = match &series.series.scope {
				Scope::Directory(directory) => report::directory(directory),
				other => report::scope(other),
			};
			format!("{scope}: {}", series.series.name)
		}
	}
}

/// The command to register an unclaimed successor and its predecessor.
pub(crate) fn inheritance_hint(
	workspace: &crate::workspace::Workspace,
	subject: &Subject,
) -> Option<String> {
	let SubjectKind::Unclaimed(file) = &subject.kind else {
		return None;
	};
	let predecessor = file.inherit_from.as_ref()?;
	let mut command = vec!["source-sentry".to_owned()];
	if let Some(repo) = workspace.repo_argument() {
		command.extend(["--repo".to_owned(), repo.display().to_string()]);
	}
	let path = workspace.display_path(&file.path);
	command.extend([
		"track".to_owned(),
		if path.starts_with('-') {
			format!("./{path}")
		} else {
			path
		},
	]);
	let command = command
		.iter()
		.map(|word| crate::commands::shell_word(word))
		.collect::<Vec<_>>()
		.join(" ");
	Some(format!(
		"Run `{command}` to inherit the declaration of `{predecessor}`"
	))
}

/// A subject's facts, as one line: `stale, overdue (due 2026-11-03)`.
/// `fresh` is left out, so a passing subject reads `fresh`.
pub(crate) fn facts(subject: &Subject) -> String {
	let mut phrases = Vec::new();
	for fact in &subject.facts {
		let phrase = match fact {
			Fact::Fresh => continue,
			Fact::Overdue => match &subject.deadline {
				Some(Deadline { due: Some(due), .. }) => format!("overdue (due {due})"),
				Some(Deadline {
					due: None,
					overdue_from,
				}) => format!("overdue (since {})", date(*overdue_from)),
				None => "overdue".to_owned(),
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

/// The date an unattested document's clock started, when it did.
fn unattested_since(subject: &Subject) -> Option<String> {
	let document = subject.document()?;
	let DocumentState::Live(live) = &document.state else {
		return None;
	};
	match live.unattested.as_ref()? {
		Timing::Started(clock) => Some(date(clock.start.timestamp)),
		Timing::Idle | Timing::NotStarted | Timing::Unknown | Timing::Failed => {
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
pub(crate) fn source_change(source: &SourceReport) -> String {
	let pattern = source.pattern.as_deref().unwrap_or("its pattern");
	let what = match source.state {
		SourceState::Fresh => format!("{} is unchanged", source.path),
		SourceState::Drifted => format!("{} changed", source.path),
		SourceState::Unattested => format!("{} was never attested", source.path),
		SourceState::Added => format!("{} newly matches {pattern}", source.path),
		SourceState::Removed => {
			format!("{} no longer matches {pattern}", source.path)
		}
		SourceState::Missing => format!("{} is missing", source.path),
		SourceState::Unhashable => format!("{} cannot be hashed", source.path),
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
		Timing::NotStarted => " in the working tree".to_owned(),
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
/// its facts already say.
pub(crate) fn changes(subject: &Subject) -> Vec<String> {
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
		.map(source_change)
		.chain(
			live
				.stale_upstreams
				.iter()
				.map(|upstream| format!("its source {upstream} is not up to date")),
		)
		.collect()
}

/// The rules that decided a subject's outcome, one line each, with the
/// facts each decided: `production: privacy + stale, overdue → fail
/// (.source-sentry/source-sentry.yaml:20)`. A fallback decides without a
/// rule, so it has no line.
pub(crate) fn deciding_rules(
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
		let origin = rule_origin(&applied.origin);
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
pub(crate) fn rule_origin(origin: &RuleOrigin) -> String {
	match origin {
		RuleOrigin::Context {
			definition: Some(definition),
			..
		}
		| RuleOrigin::Scoped(definition) => config_origin(definition),
		RuleOrigin::Context {
			definition: None, ..
		} => "built-in".to_owned(),
	}
}

/// Where a config value comes from: `docs/source-sentry.yaml:7`, with the
/// presets it was imported through, or `built-in`.
pub(crate) fn config_origin(origin: &Origin) -> String {
	let Some(file) = &origin.file else {
		return "built-in".to_owned();
	};
	let mut text = file.to_string();
	if let Some(position) = origin.position {
		let _ = write!(text, ":{}", position.line);
	}
	if !origin.import_chain.is_empty() {
		let chain: Vec<String> = origin
			.import_chain
			.iter()
			.map(ToString::to_string)
			.collect();
		let _ = write!(text, ", imported through {}", chain.join(" → "));
	}
	text
}

/// Where an effective setting comes from, with the scope of a config.
pub(crate) fn setting_origin(origin: &SettingOrigin) -> String {
	match origin {
		SettingOrigin::BuiltIn => "built-in".to_owned(),
		SettingOrigin::Config(origin) if origin.file.is_none() => {
			"built-in".to_owned()
		}
		SettingOrigin::Config(origin) => format!(
			"{} (for {})",
			config_origin(origin),
			report::scope(&origin.scope)
		),
		SettingOrigin::Declaration { file, field } => format!("{file} `{field}`"),
	}
}

/// Where a diagnostic points: `file:line:column`, or the file and field
/// when the line is unknown.
pub(crate) fn location(location: &Location) -> String {
	let mut text = location.file.to_string();
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

/// A deadline in full: `due 2026-11-03, overdue from 2026-11-04 00:00 UTC`.
pub(crate) fn deadline(deadline: &Deadline) -> String {
	match deadline.due {
		Some(due) => {
			format!("due {due}, overdue from {}", instant(deadline.overdue_from))
		}
		None => format!(
			"due at once, overdue from {}",
			instant(deadline.overdue_from)
		),
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
