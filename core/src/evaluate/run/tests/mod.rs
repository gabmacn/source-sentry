//! Tests of [`check`], end to end: fake histories and snapshots, real configs
//! and real discovery.

mod documents;
mod policy;
mod records;
mod series;

use chrono::{DateTime, TimeDelta, TimeZone as _, Utc};
use serde_json::{Value, json};

use super::{Evaluation, Subject, Timing, check};
use crate::claims::{DiagnosticKind, discover};
use crate::config::ConfigSet;
use crate::model::{DocumentId, Fact, Outcome};
use crate::policy::ExitCode;
use crate::ports::CommitId;
use crate::testing::{FakeHistory, FakeSnapshot, fake_oid, path};

/// The repository config's path.
const REPOSITORY: &str = ".source-sentry/source-sentry.yaml";

/// The lockfile of `docs/`.
const DOCS_LOCK: &str = "docs/source-sentry.lock";

/// A repository config with two classes and a `production` context that
/// blocks privacy documents.
const CONFIG: &str = "\
version: 1
define:
  classes:
    internal-docs: {}
    privacy: {}
  contexts:
    production:
      fallback: warn
      rules:
        - facts: ['missed']
          outcome: notice
        - classes: ['privacy']
          facts: ['stale', 'overdue', 'unattested', 'timing-unknown', \
          'upstream-stale', 'unclaimed']
          outcome: fail
";

/// Midnight UTC, `n` days after 1 January 2026.
fn day(n: i64) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + TimeDelta::days(n)
}

/// The fake oid of `content`, as lockfiles write it.
fn oid(content: &str) -> String {
	fake_oid(content.as_bytes()).to_string()
}

/// A review, in JSON.
fn review() -> Value {
	json!({"at": "2026-01-01T00:00:00Z", "by": null})
}

/// A path source attested at the oid of `content`.
fn attested(source: &str, content: &str) -> Value {
	json!({"path": source, "oid": oid(content), "reviewed": review()})
}

/// A seal at the oid of `content`.
fn seal(content: &str) -> Value {
	json!({"at": "2026-01-01T00:00:00Z", "by": null, "oid": oid(content)})
}

/// A lockfile with `documents`, a JSON object keyed by relative path or
/// identity, and no tombstones.
fn lockfile(documents: &Value) -> String {
	serde_json::to_string_pretty(
		&json!({"documents": documents, "retired": {}, "schema": 1}),
	)
	.unwrap()
}

/// A history built one day at a time.
struct Repo {
	history: FakeHistory,
}

impl Repo {
	/// A history whose first commit, on day 0, holds `files`.
	fn new(files: &[(&str, &str)]) -> Self {
		let mut repo = Self {
			history: FakeHistory::new(),
		};
		let changes: Vec<(&str, Option<&str>)> = files
			.iter()
			.map(|&(file, content)| (file, Some(content)))
			.collect();
		repo.commit(0, &changes);
		repo
	}

	/// Commits `changes` on day `n`: `Some(content)` writes a file and
	/// `None` deletes it.
	fn commit(&mut self, n: i64, changes: &[(&str, Option<&str>)]) -> CommitId {
		let changes: Vec<(&str, Option<&[u8]>)> = changes
			.iter()
			.map(|&(file, content)| (file, content.map(str::as_bytes)))
			.collect();
		self.history.commit(day(n), &changes)
	}

	/// A clean checkout of the head.
	fn checkout(&self) -> FakeSnapshot {
		FakeSnapshot::checkout(&self.history)
	}

	/// Evaluates a clean checkout at `at` under `context`.
	fn evaluate(&self, at: DateTime<Utc>, context: Option<&str>) -> Evaluation {
		evaluate(&self.history, &self.checkout(), at, context)
	}
}

/// Loads the configs of `snapshot`, discovers and evaluates it.
fn evaluate(
	history: &FakeHistory,
	snapshot: &FakeSnapshot,
	at: DateTime<Utc>,
	context: Option<&str>,
) -> Evaluation {
	let configs = match ConfigSet::load(snapshot) {
		Ok(configs) => configs,
		Err(error) => panic!("expected valid configs, got:\n{error}"),
	};
	let inventory = discover(snapshot, &configs).unwrap();
	check(snapshot, history, &configs, &inventory, at, context).unwrap()
}

/// The only subject of the local document `file`.
fn document<'a>(evaluation: &'a Evaluation, file: &str) -> &'a Subject {
	let id = DocumentId::Local(path(file));
	let subjects: Vec<&Subject> = evaluation.subjects_of(&id).collect();
	let [subject] = subjects.as_slice() else {
		panic!("expected one subject for {file}, got {subjects:#?}");
	};
	subject
}

/// A subject's facts, in order.
fn facts(subject: &Subject) -> Vec<Fact> {
	subject.facts.iter().copied().collect()
}

/// The kinds of every diagnostic, in order.
fn kinds(evaluation: &Evaluation) -> Vec<DiagnosticKind> {
	evaluation
		.diagnostics
		.iter()
		.map(|diagnostic| diagnostic.kind)
		.collect()
}

/// Asserts that the evaluation found nothing wrong.
fn assert_clean(evaluation: &Evaluation) {
	assert!(
		evaluation.diagnostics.is_empty(),
		"expected no diagnostics, got {:#?}",
		evaluation.diagnostics
	);
}

/// The exit codes of the report gate and the continue gate.
fn gates(evaluation: &Evaluation) -> (ExitCode, ExitCode) {
	let gates = evaluation.gates();
	(gates.report, gates.proceed)
}

/// The timing of the source at `source` of the only subject of `file`.
fn timing<'a>(
	evaluation: &'a Evaluation,
	file: &str,
	source: &str,
) -> &'a Timing {
	let subject = document(evaluation, file);
	let Some(super::DocumentState::Live(live)) =
		subject.document().map(|document| &document.state)
	else {
		panic!("{file} is not a live document: {subject:#?}");
	};
	live
		.sources
		.iter()
		.find(|report| report.path == path(source))
		.map_or_else(
			|| panic!("{file} has no source {source}: {live:#?}"),
			|report| &report.timing,
		)
}

/// The commit and timestamp a started timing records.
fn started(timing: &Timing) -> (&CommitId, DateTime<Utc>) {
	let Timing::Started(clock) = timing else {
		panic!("expected a started clock, got {timing:?}");
	};
	(&clock.start.commit, clock.start.timestamp)
}

/// The outcome of `subject`, which must have been judged.
fn outcome(subject: &Subject) -> Outcome {
	subject
		.outcome()
		.unwrap_or_else(|| panic!("expected a judgement: {subject:#?}"))
}
