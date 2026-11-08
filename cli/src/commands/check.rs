//! `check`: evaluate a snapshot and exit with the gate's code.

use std::fmt::Write as _;

use source_sentry_core::evaluate::{Evaluation, Subject, check};
use source_sentry_core::model::Outcome;
use source_sentry_core::policy::{ErrorKind, Threshold};
use source_sentry_core::ports::Snapshot;
use source_sentry_core::report::{CheckReport, to_json};
use source_sentry_git::GitRepository;

use crate::cli::{CheckArgs, EvaluationArgs, Format};
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `check`. The exit code is the evaluation's under `--fail-on`.
/// `--no-cache` changes nothing yet, because no cache exists.
///
/// # Errors
///
/// Exit 3 when the configs cannot load, and 4 when the repository cannot be
/// read or the evaluation cannot run.
pub(super) fn run(
	workspace: &Workspace,
	args: &CheckArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let evaluation = evaluate(workspace, &args.evaluation)?;
	let threshold = Threshold::from(args.fail_on);
	match args.format {
		Format::Json => {
			let report = CheckReport::new(&evaluation, threshold);
			let json = to_json(&report).map_err(|error| {
				Failure::new(
					exit::OPERATIONAL,
					format!("cannot write the report: {error}"),
				)
			})?;
			output::data(&json);
		}
		Format::Human if printer.is_quiet() => {
			for diagnostic in &evaluation.diagnostics {
				if diagnostic.severity().is_fatal() {
					output::write_err(&diagnostic_line(diagnostic));
				}
			}
		}
		Format::Human => printer.print(&render(&evaluation, threshold)),
	}
	Ok(evaluation.exit_code(threshold).code())
}

/// Evaluates the snapshot that `args` selects, at `--at` or now, under
/// `--context`.
///
/// # Errors
///
/// Exit 3 when the configs cannot load, and 4 when the repository cannot be
/// read.
pub(super) fn evaluate(
	workspace: &Workspace,
	args: &EvaluationArgs,
) -> Result<Evaluation, Failure> {
	let at = args.at.unwrap_or_else(super::now);
	let repository = &workspace.repository;
	if let Some(revision) = &args.rev {
		let snapshot = repository
			.commit_snapshot(revision)
			.map_err(|error| Failure::backend(&error))?;
		evaluate_snapshot(&snapshot, repository, at, &args.context)
	} else {
		let snapshot = repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		evaluate_snapshot(&snapshot, repository, at, &args.context)
	}
}

fn evaluate_snapshot<S: Snapshot>(
	snapshot: &S,
	repository: &GitRepository,
	at: chrono::DateTime<chrono::Utc>,
	context: &str,
) -> Result<Evaluation, Failure> {
	let (configs, inventory) = load(snapshot)?;
	check(
		snapshot,
		repository,
		&configs,
		&inventory,
		at,
		Some(context),
	)
	.map_err(|error| Failure::backend(&error))
}

/// The human report, as `docs/cli.md` sketches it: the header, every subject
/// that did not pass with its reasons, the diagnostics, and the summary.
pub(super) fn render(evaluation: &Evaluation, threshold: Threshold) -> String {
	let mut text = describe::header(evaluation);
	text.push('\n');

	let mut listed: Vec<(&Subject, Outcome)> = evaluation
		.subjects
		.iter()
		.filter_map(|subject| Some((subject, subject.outcome()?)))
		.filter(|(_, outcome)| *outcome != Outcome::Pass)
		.collect();
	// The most severe first; the evaluation's order within each outcome.
	listed.sort_by_key(|(_, outcome)| std::cmp::Reverse(*outcome));
	let names: Vec<String> = listed
		.iter()
		.map(|(subject, _)| describe::subject_name(subject))
		.collect();
	let width = names
		.iter()
		.map(|name| name.chars().count())
		.max()
		.unwrap_or(0)
		.min(40);
	if !listed.is_empty() {
		text.push('\n');
	}
	for ((subject, outcome), name) in listed.iter().zip(&names) {
		let _ = writeln!(
			text,
			"{}  {}  {}",
			output::outcome_label(*outcome),
			output::pad(&output::bold(name), name, width),
			describe::facts(subject)
		);
		let details = describe::changes(subject)
			.into_iter()
			.chain(describe::deciding_rules(evaluation, subject));
		for detail in details {
			let _ = writeln!(text, "      {}", output::dim(&detail));
		}
	}

	if !evaluation.diagnostics.is_empty() {
		text.push('\n');
		for diagnostic in &evaluation.diagnostics {
			let _ = writeln!(text, "{}", diagnostic_line(diagnostic));
		}
	}

	text.push('\n');
	text.push_str(&summary(evaluation, threshold));
	text
}

/// One diagnostic: its severity, its code, where it points and its message.
fn diagnostic_line(
	diagnostic: &source_sentry_core::claims::Diagnostic,
) -> String {
	let severity = diagnostic.severity();
	format!(
		"{}  {}  {}  {}",
		output::severity(severity.name(), severity.is_fatal()),
		diagnostic.kind.code(),
		describe::location(&diagnostic.location),
		diagnostic.message
	)
}

/// The last line: `1 failing · 1 warning · 1 notice · 9 passing · exit 2`.
fn summary(evaluation: &Evaluation, threshold: Threshold) -> String {
	let counts = &evaluation.summary;
	let mut parts = Vec::new();
	for (outcome, singular, plural) in [
		(Outcome::Fail, "failing", "failing"),
		(Outcome::Warn, "warning", "warnings"),
		(Outcome::Notice, "notice", "notices"),
	] {
		let count = counts.outcomes(outcome);
		if count > 0 {
			parts.push(describe::count(count, singular, plural));
		}
	}
	parts.push(describe::count(
		counts.outcomes(Outcome::Pass),
		"passing",
		"passing",
	));
	for (kind, singular, plural) in [
		(
			ErrorKind::Configuration,
			"configuration error",
			"configuration errors",
		),
		(ErrorKind::Integrity, "integrity error", "integrity errors"),
		(
			ErrorKind::Operational,
			"operational error",
			"operational errors",
		),
	] {
		let count = counts.errors(kind);
		if count > 0 {
			parts.push(describe::count(count, singular, plural));
		}
	}
	parts.push(format!("exit {}", evaluation.exit_code(threshold).code()));
	parts.join(" · ")
}
