//! `check`: evaluate a snapshot and exit with the gate's code.

use std::fmt::Write as _;

use source_sentry_core::claims::{
	Diagnostic, Outside, discover, outside_the_index,
};
use source_sentry_core::config::{ConfigSet, LoadError};
use source_sentry_core::evaluate::{
	Evaluation, Subject, SubjectKind, check, unloadable,
};
use source_sentry_core::model::{Outcome, RepoPath};
use source_sentry_core::policy::{ErrorKind, Threshold};
use source_sentry_core::ports::Snapshot;
use source_sentry_core::report::{CheckReport, to_json};
use source_sentry_git::GitRepository;

use crate::cli::{CheckArgs, EvaluationArgs, Format};
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::Workspace;

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
	// A JSON report is written even when the configs cannot load.
	let report_invalid = args.format == Format::Json;
	let (evaluation, configs) =
		evaluate_with_configs(workspace, &args.evaluation, report_invalid)?;
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
					output::write_err(&diagnostic_line(workspace, diagnostic));
				}
			}
		}
		Format::Human => {
			// Only the working tree has files outside the index.
			let outside = match &configs {
				Some(configs) if args.evaluation.rev.is_none() => {
					let untracked = workspace
						.repository
						.untracked_files()
						.map_err(|error| Failure::backend(&error))?;
					outside_the_index(&untracked, configs)
				}
				_ => Vec::new(),
			};
			printer.print(&render(workspace, &evaluation, threshold, &outside));
		}
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
	evaluate_with_configs(workspace, args, false)
		.map(|(evaluation, _)| evaluation)
}

/// [`evaluate`], also returning the configs it loaded. With
/// `report_invalid`, configs that cannot load give an evaluation that
/// reports their errors (exit 3), and no configs, instead of a failure.
fn evaluate_with_configs(
	workspace: &Workspace,
	args: &EvaluationArgs,
	report_invalid: bool,
) -> Result<(Evaluation, Option<ConfigSet>), Failure> {
	let at = args.at.unwrap_or_else(super::now);
	let repository = &workspace.repository;
	if let Some(revision) = &args.rev {
		let snapshot = repository
			.commit_snapshot(revision)
			.map_err(|error| Failure::backend(&error))?;
		evaluate_snapshot(
			workspace,
			&snapshot,
			repository,
			at,
			&args.context,
			report_invalid,
		)
	} else {
		let snapshot = repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		evaluate_snapshot(
			workspace,
			&snapshot,
			repository,
			at,
			&args.context,
			report_invalid,
		)
	}
}

fn evaluate_snapshot<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
	repository: &GitRepository,
	at: chrono::DateTime<chrono::Utc>,
	context: &str,
	report_invalid: bool,
) -> Result<(Evaluation, Option<ConfigSet>), Failure> {
	let configs = match ConfigSet::load(snapshot) {
		Ok(configs) => configs,
		Err(LoadError::Invalid(errors)) if report_invalid => {
			return Ok((unloadable(snapshot.revision(), at, &errors), None));
		}
		Err(error) => return Err(Failure::load(workspace, &error)),
	};
	let inventory =
		discover(snapshot, &configs).map_err(|error| Failure::backend(&error))?;
	let evaluation = check(
		snapshot,
		repository,
		&configs,
		&inventory,
		at,
		Some(context),
	)
	.map_err(|error| Failure::backend(&error))?;
	Ok((evaluation, Some(configs)))
}

/// The human report, as `docs/cli.md` sketches it: the header, every subject
/// that did not pass with its reasons, the diagnostics, and the summary.
///
/// A document's own diagnostics, such as a missing source, are shown under
/// it, and it is listed for them even when it passes, so that a passing
/// line never hides an error about the same document.
pub(super) fn render(
	workspace: &Workspace,
	evaluation: &Evaluation,
	threshold: Threshold,
	outside: &[(RepoPath, Outside)],
) -> String {
	let mut text = describe::header(evaluation);
	text.push('\n');

	let mut shown = vec![false; evaluation.diagnostics.len()];
	let mut listed: Vec<(&Subject, Outcome, Vec<&Diagnostic>)> = Vec::new();
	for subject in &evaluation.subjects {
		let Some(outcome) = subject.outcome() else {
			continue;
		};
		let mut own = Vec::new();
		if let SubjectKind::Document(document) = &subject.kind {
			for (index, diagnostic) in evaluation.diagnostics.iter().enumerate() {
				if !shown[index]
					&& diagnostic.document.as_ref() == Some(&document.document)
				{
					shown[index] = true;
					own.push(diagnostic);
				}
			}
		}
		if outcome != Outcome::Pass
			|| !own.is_empty()
			|| matches!(&subject.kind, SubjectKind::Unclaimed(file) if file.inherit_from.is_some())
		{
			listed.push((subject, outcome, own));
		}
	}
	// The most severe first; the evaluation's order within each outcome.
	listed.sort_by_key(|(_, outcome, _)| std::cmp::Reverse(*outcome));
	let names: Vec<String> = listed
		.iter()
		.map(|(subject, _, _)| describe::subject_name(workspace, subject))
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
	for ((subject, outcome, own), name) in listed.iter().zip(&names) {
		let _ = writeln!(
			text,
			"{}  {}  {}",
			output::outcome_label(*outcome),
			output::pad(&output::bold(name), name, width),
			describe::facts(subject)
		);
		let details = describe::changes(workspace, subject)
			.into_iter()
			.chain(describe::deciding_rules(workspace, evaluation, subject));
		for detail in details {
			let _ = writeln!(text, "      {}", output::dim(&detail));
		}
		if let Some(hint) = describe::inheritance_hint(workspace, subject) {
			let _ = writeln!(text, "      {hint}");
		}
		for diagnostic in own {
			let _ =
				writeln!(text, "      {}", diagnostic_line(workspace, diagnostic));
		}
	}

	let rest: Vec<&Diagnostic> = evaluation
		.diagnostics
		.iter()
		.zip(&shown)
		.filter(|(_, shown)| !**shown)
		.map(|(diagnostic, _)| diagnostic)
		.collect();
	if !rest.is_empty() {
		text.push('\n');
		for diagnostic in rest {
			let _ = writeln!(text, "{}", diagnostic_line(workspace, diagnostic));
		}
	}

	if !outside.is_empty() {
		text.push('\n');
		text.push_str(&outside_note(workspace, outside));
	}

	text.push('\n');
	text.push_str(&summary(evaluation, threshold));
	text
}

/// The most files the note on files outside the index names.
const OUTSIDE_SHOWN: usize = 10;

/// The note on files that would count but that git does not track, which
/// `check` therefore leaves out. It never changes the exit code.
fn outside_note(
	workspace: &Workspace,
	outside: &[(RepoPath, Outside)],
) -> String {
	let mut text = format!(
		"{}  {} outside git's index, so this check leaves {} out; `git add` {} \
		 to include {}:\n",
		output::severity("note", false),
		describe::count(
			outside.len(),
			"file would count but is",
			"files would count but are"
		),
		if outside.len() == 1 { "it" } else { "them" },
		if outside.len() == 1 { "it" } else { "them" },
		if outside.len() == 1 { "it" } else { "them" },
	);
	for (path, kind) in outside.iter().take(OUTSIDE_SHOWN) {
		let what = match kind {
			Outside::Own => "Source Sentry's own file",
			Outside::Document => "a document",
		};
		let _ = writeln!(
			text,
			"      {}  {}",
			workspace.display_path(path),
			output::dim(what)
		);
	}
	if outside.len() > OUTSIDE_SHOWN {
		let _ = writeln!(text, "      and {} more", outside.len() - OUTSIDE_SHOWN);
	}
	text
}

/// One diagnostic: its severity, its code, where it points and its message.
pub(super) fn diagnostic_line(
	workspace: &Workspace,
	diagnostic: &source_sentry_core::claims::Diagnostic,
) -> String {
	let severity = diagnostic.severity();
	format!(
		"{}  {}  {}  {}",
		output::severity(severity.name(), severity.is_fatal()),
		diagnostic.kind.code(),
		describe::location(workspace, &diagnostic.location),
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
