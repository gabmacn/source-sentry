//! `explain`: one subject in full: its effective
//! settings and where each comes from, its facts, the rules that matched,
//! the timing evidence and its next deadline.

use std::fmt::Write as _;

use source_sentry_core::claims::{Diagnostic, Mode, Severity};
use source_sentry_core::evaluate::{
	CompletionState, DocumentState, DocumentSubject, Evaluation, LiveDocument,
	RecordCheck, RecordState, SeriesSubject, SourceState, Subject, SubjectKind,
	Timing,
};
use source_sentry_core::model::{
	ClassName, DocumentId, Lifecycle, ObjectId, VersionKind,
};
use source_sentry_core::policy::Basis;

use crate::cli::{ExplainArgs, parse_document};
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::Workspace;

/// How many cadence windows to list, newest last.
const WINDOWS_SHOWN: usize = 12;

/// Runs `explain`. Every subject the argument names is explained: one
/// normally, one per claim for a document claimed twice, and one per scope
/// for a series name that several scopes define.
///
/// # Errors
///
/// Exit 2 when nothing evaluated matches the argument, and the evaluation's
/// failures otherwise.
pub(super) fn run(
	workspace: &Workspace,
	args: &ExplainArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let evaluation = super::check::evaluate(workspace, &args.evaluation)?;
	let subjects = find(workspace, &evaluation, &args.subject)?;
	let mut code = exit::SUCCESS;
	let explanations: Vec<String> = subjects
		.iter()
		.map(|subject| {
			let own = own_diagnostics(&evaluation, subject);
			for diagnostic in &own {
				code = code.max(match diagnostic.severity() {
					Severity::Warning => exit::SUCCESS,
					Severity::Configuration | Severity::Integrity => exit::CONFIGURATION,
					Severity::Operational => exit::OPERATIONAL,
				});
			}
			let mut text = explain(workspace, &evaluation, subject);
			if !own.is_empty() {
				heading(&mut text, "Diagnostics");
				for diagnostic in own {
					let _ = writeln!(
						text,
						"  {}",
						super::check::diagnostic_line(workspace, diagnostic)
					);
				}
			}
			text
		})
		.collect();
	printer.print(&explanations.join("\n"));
	Ok(code)
}

/// The diagnostics about `subject`'s document, such as a clock that could
/// not be dated, which `check` lists among all the others.
fn own_diagnostics<'a>(
	evaluation: &'a Evaluation,
	subject: &Subject,
) -> Vec<&'a Diagnostic> {
	let Some(document) = subject.document() else {
		return Vec::new();
	};
	evaluation
		.diagnostics
		.iter()
		.filter(|diagnostic| {
			diagnostic.document.as_ref() == Some(&document.document)
		})
		.collect()
}

/// The subjects that `argument` names: a document by identity or path, an
/// unclaimed file by path, or else a series by name.
fn find<'a>(
	workspace: &Workspace,
	evaluation: &'a Evaluation,
	argument: &str,
) -> Result<Vec<&'a Subject>, Failure> {
	// A document or file, named as every command names documents; a parse
	// or path failure is kept, in case the argument names a series instead.
	let by_path = parse_document(argument)
		.map_err(Failure::usage)
		.and_then(|document| workspace.document(&document))
		.map(|document| {
			evaluation
				.subjects
				.iter()
				.filter(|subject| match &subject.kind {
					SubjectKind::Document(found) => found.document == document,
					SubjectKind::Unclaimed(file) => {
						document == DocumentId::Local(file.path.clone())
					}
					SubjectKind::Series(_) => false,
				})
				.collect::<Vec<_>>()
		});
	if let Ok(found) = &by_path
		&& !found.is_empty()
	{
		return by_path;
	}
	let series: Vec<&Subject> = evaluation
		.subjects
		.iter()
		.filter(|subject| {
			subject
				.series()
				.is_some_and(|series| series.series.name == argument)
		})
		.collect();
	if !series.is_empty() {
		return Ok(series);
	}
	by_path?;
	Err(Failure::new(
		exit::REFUSED,
		format!(
			"nothing named `{argument}` was evaluated: it is not a tracked \
			 document, an unclaimed file or a series"
		),
	))
}

/// One subject in full.
fn explain(
	workspace: &Workspace,
	evaluation: &Evaluation,
	subject: &Subject,
) -> String {
	let mut text = output::bold(&describe::subject_name(workspace, subject));
	text.push('\n');
	text.push_str(&output::dim(&describe::header(evaluation)));
	text.push_str("\n\n");

	let mut rows: Vec<(&str, String)> = Vec::new();
	match &subject.kind {
		SubjectKind::Document(document) => {
			document_rows(workspace, document, &mut rows);
		}
		SubjectKind::Unclaimed(file) => rows.push((
			"File",
			format!(
				"unclaimed, found by the scan of root {}",
				workspace.display_path(&file.root)
			),
		)),
		SubjectKind::Series(series) => series_rows(workspace, series, &mut rows),
	}
	rows.push((
		"Outcome",
		subject.outcome().map_or_else(
			|| {
				"not judged: no context could be selected, or an error stands in"
					.to_owned()
			},
			output::outcome_label,
		),
	));
	rows.push(("Facts", describe::facts(subject)));
	rows.push((
		"Deadline",
		subject
			.deadline
			.as_ref()
			.map_or_else(|| "none: no clock runs".to_owned(), describe::deadline),
	));
	table(&mut text, &rows);
	if let Some(hint) = describe::inheritance_hint(workspace, subject) {
		let _ = writeln!(text, "\n{hint}");
	}

	heading(&mut text, "Settings");
	let settings = &subject.settings;
	let classes: Vec<&str> = settings
		.classes
		.value
		.iter()
		.map(ClassName::as_str)
		.collect();
	let cadence = settings.cadence.value.as_ref().map_or_else(
		|| "none".to_owned(),
		|cadence| format!("{} from {}", cadence.rrule, cadence.dtstart),
	);
	let values = [
		("classes", list(&classes), &settings.classes.origin),
		(
			"owners",
			list(&settings.owners.value),
			&settings.owners.origin,
		),
		(
			"grace",
			describe::count(settings.grace.value as usize, "day", "days"),
			&settings.grace.origin,
		),
		("cadence", cadence, &settings.cadence.origin),
	];
	let width = values
		.iter()
		.map(|(_, value, _)| value.chars().count())
		.max()
		.unwrap_or(0);
	for (name, value, origin) in values {
		let _ = writeln!(
			text,
			"  {name:<8} {}  {}",
			output::pad(&value, &value, width),
			output::dim(&describe::setting_origin(workspace, origin))
		);
	}

	match &subject.kind {
		SubjectKind::Document(DocumentSubject {
			state: DocumentState::Live(live),
			..
		}) => sources(workspace, &mut text, live),
		SubjectKind::Series(series) => members(workspace, &mut text, series),
		SubjectKind::Document(_) | SubjectKind::Unclaimed(_) => {}
	}
	judgement(workspace, &mut text, evaluation, subject);
	text
}

/// The rows that describe a document's claim and state.
fn document_rows(
	workspace: &Workspace,
	document: &DocumentSubject,
	rows: &mut Vec<(&str, String)>,
) {
	rows.push((
		"Document",
		format!(
			"tracked {} in {} `{}`",
			match document.mode {
				Mode::OnFile => "on-file",
				Mode::LocalLockfile => "by its lockfile",
				Mode::RemoteLockfile { .. } => "by its owner lockfile",
			},
			workspace.display_path(&document.declared_in),
			document.field
		),
	));
	match &document.state {
		DocumentState::Record(record) => {
			rows.push(("Record", record_state(record)));
		}
		DocumentState::Live(live) => {
			if let Some(timing) = &live.unattested {
				rows.push((
					"Unattested",
					format!("never reviewed; declared{}", clock(timing)),
				));
			}
		}
	}
	if let Some(series) = &document.awaiting_seal {
		rows.push((
			"Awaiting",
			format!(
				"a seal: series {} ({}) has a newer entry",
				series.name,
				describe::scope(workspace, &series.scope)
			),
		));
	}
}

/// A record's check against its seal.
fn record_state(record: &RecordCheck) -> String {
	let seal = describe::short(&record.seal);
	match &record.state {
		RecordState::Intact => format!("sealed at {seal}, intact"),
		RecordState::Broken { actual } => format!(
			"sealed at {seal}, but it is now {}: the seal is broken",
			describe::short(actual)
		),
		RecordState::Missing => {
			format!("sealed at {seal}, but the file is missing")
		}
		RecordState::Unhashable => {
			format!("sealed at {seal}, but the file cannot be hashed")
		}
	}
}

/// The rows that describe a series.
fn series_rows(
	workspace: &Workspace,
	series: &SeriesSubject,
	rows: &mut Vec<(&str, String)>,
) {
	let definition = &series.series;
	rows.push((
		"Series",
		format!(
			"{}, {} versions, defined in {} (for {})",
			match definition.lifecycle {
				Lifecycle::Living => "living",
				Lifecycle::RecordsOnly => "records-only",
			},
			match definition.version_kind {
				VersionKind::Integer => "integer",
				VersionKind::Dotted => "dotted",
				VersionKind::Date => "date",
			},
			describe::config_origin(workspace, &definition.origin),
			describe::scope(workspace, &definition.scope)
		),
	));
	rows.push(("Pattern", definition.pattern.as_str().to_owned()));
	if let Some(head) = &series.head {
		rows.push(("Head", workspace.display_path(head)));
	}
}

/// A clock's evidence, as a suffix: ` in 7d2e9b0 at 2026-10-20 10:00 UTC`.
fn clock(timing: &Timing) -> String {
	match timing {
		Timing::Started(clock) => format!(
			" in {}, counted from {}{}",
			describe::short_commit(&clock.start.commit),
			describe::instant(clock.start.timestamp),
			clock
				.deadline
				.as_ref()
				.map_or_else(String::new, |deadline| format!(
					"; {}",
					describe::deadline(deadline)
				))
		),
		Timing::NotStarted => {
			" in the working tree only, so no clock has started".to_owned()
		}
		other => describe::timing_suffix(other),
	}
}

/// Each source with its oids, state and clock.
fn sources(workspace: &Workspace, text: &mut String, live: &LiveDocument) {
	heading(text, "Sources");
	if live.sources.is_empty() {
		text.push_str("  none declared\n");
	}
	for source in &live.sources {
		let pattern = source
			.pattern
			.as_ref()
			.map_or_else(String::new, |pattern| format!(", from {pattern}"));
		let _ = writeln!(
			text,
			"  {}  {}{pattern}",
			output::bold(&workspace.display_path(&source.path)),
			state_name(source.state)
		);
		let oid = |oid: Option<&ObjectId>| {
			oid.map_or_else(|| "none".to_owned(), ToString::to_string)
		};
		let _ = writeln!(
			text,
			"      attested {}, now {}",
			oid(source.attested.as_ref()),
			oid(source.current.as_ref())
		);
		let evidence = clock(&source.timing);
		if !evidence.is_empty() {
			let _ = writeln!(text, "      changed{evidence}");
		}
	}
	for upstream in &live.stale_upstreams {
		let _ = writeln!(
			text,
			"  source document {} is not up to date",
			workspace.display_path(upstream)
		);
	}
}

/// How a source compares with its attestation.
fn state_name(state: SourceState) -> &'static str {
	match state {
		SourceState::Fresh => "fresh",
		SourceState::Drifted => "drifted",
		SourceState::Unattested => "never attested",
		SourceState::Added => "added to its pattern",
		SourceState::Removed => "removed from its pattern",
		SourceState::Missing => "missing",
		SourceState::Unhashable => "cannot be hashed",
	}
}

/// A series' members and its recent cadence windows.
fn members(workspace: &Workspace, text: &mut String, series: &SeriesSubject) {
	heading(text, "Members");
	if series.members.is_empty() {
		text.push_str("  none yet\n");
	}
	let completions = series
		.cadence
		.as_ref()
		.map(|cadence| cadence.completions.as_slice())
		.unwrap_or_default();
	for member in &series.members {
		let completion = completions
			.iter()
			.filter(|completion| completion.member == member.path)
			.map(|completion| match &completion.state {
				CompletionState::Completed(start) => format!(
					", completed in {}, counted from {}",
					describe::short_commit(&start.commit),
					describe::date(start.timestamp)
				),
				CompletionState::Uncommitted => {
					", completed in the working tree only, which does not count yet"
						.to_owned()
				}
				CompletionState::Failed => {
					", completed at a time that could not be dated".to_owned()
				}
			})
			.collect::<String>();
		let _ = writeln!(
			text,
			"  {}  version {}{completion}",
			workspace.display_path(&member.path),
			member.version
		);
	}
	let Some(status) = series
		.cadence
		.as_ref()
		.and_then(|cadence| cadence.status.as_ref())
	else {
		return;
	};
	heading(text, "Windows");
	let skipped = status.windows.len().saturating_sub(WINDOWS_SHOWN);
	if skipped > 0 {
		let _ = writeln!(
			text,
			"  … {} earlier",
			describe::count(skipped, "window", "windows")
		);
	}
	for (position, window) in status.windows.iter().enumerate().skip(skipped) {
		let state = if let Some(credit) = &window.credit {
			format!("credited to {}", workspace.display_path(&credit.entry))
		} else if status.missed.contains(&position) {
			"missed".to_owned()
		} else if window.closed {
			"closed without an entry: overdue".to_owned()
		} else {
			"open".to_owned()
		};
		let after = window.after.map_or_else(
			|| "from the start".to_owned(),
			|after| format!("after {after}"),
		);
		let _ = writeln!(
			text,
			"  {after}, due {}, last day {}: {state}",
			window.due, window.last_day
		);
	}
}

/// Each fact's outcome, with the rule that decided it and every other rule
/// that matched.
fn judgement(
	workspace: &Workspace,
	text: &mut String,
	evaluation: &Evaluation,
	subject: &Subject,
) {
	heading(text, "Judgement");
	let Some(judgement) = &subject.judgement else {
		text.push_str(
			"  not judged: no context could be selected, or an error stands in \
			 for the outcome\n",
		);
		return;
	};
	let context = evaluation
		.context
		.as_ref()
		.map_or("the context", |context| context.selected.name.as_str());
	let width = judgement
		.facts
		.iter()
		.map(|fact| fact.fact.name().len())
		.max()
		.unwrap_or(0);
	for fact in &judgement.facts {
		let basis = match fact.basis {
			Basis::Fresh => "fresh always passes".to_owned(),
			Basis::Fallback => {
				format!("no rule matches, so the fallback of {context} applies")
			}
			Basis::Rule(reference) => {
				evaluation.rule(subject, reference).map_or_else(
					|| "a rule that cannot be found".to_owned(),
					|applied| {
						format!(
							"{context}: {} ({})",
							describe::rule(&applied.rule),
							describe::rule_origin(workspace, &applied.origin)
						)
					},
				)
			}
		};
		let _ = writeln!(
			text,
			"  {:<width$}  {}  {basis}",
			fact.fact.name(),
			output::outcome_label(fact.outcome)
		);
		for reference in &fact.matching {
			if Basis::Rule(*reference) == fact.basis {
				continue;
			}
			if let Some(applied) = evaluation.rule(subject, *reference) {
				let _ = writeln!(
					text,
					"  {:<width$}        {}",
					"",
					output::dim(&format!(
						"also matches {} ({})",
						describe::rule(&applied.rule),
						describe::rule_origin(workspace, &applied.origin)
					))
				);
			}
		}
	}
}

/// A section's heading, after a blank line.
fn heading(text: &mut String, name: &str) {
	let _ = write!(text, "\n{}\n", output::bold(name));
}

/// Rows of `name  value`, aligned.
fn table(text: &mut String, rows: &[(&str, String)]) {
	let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
	for (name, value) in rows {
		let _ = writeln!(text, "{name:<width$}  {value}");
	}
}

/// Items separated by commas, or `none`.
fn list<T: AsRef<str>>(items: &[T]) -> String {
	if items.is_empty() {
		"none".to_owned()
	} else {
		items
			.iter()
			.map(AsRef::as_ref)
			.collect::<Vec<_>>()
			.join(", ")
	}
}
