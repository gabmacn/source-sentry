//! Source documents in other projects: what is read of the other project,
//! and which of its errors reach the citing project's report.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{
	CONFIG, DOCS_LOCK, REPOSITORY, Repo, assert_clean, attested, day, document,
	evaluate, facts, gates, kinds, lockfile, outcome, review,
};
use crate::claims::{DiagnosticKind, discover};
use crate::config::ConfigSet;
use crate::evaluate::run::projects::narrowed;
use crate::evaluate::run::{Coverage, Evaluation, check, check_local};
use crate::model::{DocumentId, Fact, Outcome};
use crate::policy::ExitCode;
use crate::ports::{BackendError, BackendErrorKind, Canonical};
use crate::testing::{FakeHistory, FakeSnapshot, fake_oid, path};

/// The other project's config.
const UP_CONFIG: &str = "up/.source-sentry/source-sentry.yaml";

/// The other project's top lockfile.
const UP_LOCK: &str = "up/source-sentry.lock";

/// The other project's own document, which `docs/guide.md` cites.
const UP_GUIDE: &str = "# Upstream guide\n";

/// The lockfile entry of a reviewed document whose only source is `source`,
/// attested at `content`.
fn reviewed(source: &str, content: &str) -> Value {
	json!({"reviewed": review(), "sources": [attested(source, content)]})
}

/// A repository with the project `up`, whose config is `up_config` and whose
/// top lockfile lists `up_documents`, and whose files include `files`. The
/// root project's `docs/guide.md` cites each of `cited`, a path in `up`,
/// attested at its current content in `files`. `up/guide.md`, with the
/// content [`UP_GUIDE`], and its source `up/code.rs`, at `v1`, are always
/// there.
fn citing(
	up_config: &str,
	up_documents: &Value,
	cited: &[&str],
	files: &[(&str, &str)],
) -> Repo {
	let mut all: Vec<(&str, &str)> = vec![
		(REPOSITORY, CONFIG),
		(UP_CONFIG, up_config),
		("up/guide.md", UP_GUIDE),
		("up/code.rs", "v1"),
		("docs/guide.md", "# Guide\n"),
	];
	all.extend_from_slice(files);
	let sources: Vec<Value> = cited
		.iter()
		.map(|cited| {
			let content = all
				.iter()
				.find(|(file, _)| file == cited)
				.map_or_else(|| panic!("no file {cited}"), |(_, content)| *content);
			attested(&format!("/{cited}"), content)
		})
		.collect();
	let root = lockfile(&json!({"guide.md": {
		"reviewed": review(), "sources": sources,
	}}));
	let up = lockfile(up_documents);
	all.push((DOCS_LOCK, &root));
	all.push((UP_LOCK, &up));
	Repo::new(&all)
}

/// The usual other project: `up/guide.md` reviewed against `up/code.rs`.
fn up_guide() -> Value {
	json!({"guide.md": reviewed("/code.rs", "v1")})
}

/// A check of the project containing `start`, as `-C <start> check` runs it.
fn check_project(
	history: &FakeHistory,
	snapshot: &FakeSnapshot,
	start: &str,
	at: jiff::Timestamp,
) -> Result<Evaluation, BackendError> {
	let configs = ConfigSet::load_project(snapshot, &path(start)).unwrap();
	let inventory = discover(snapshot, &configs).unwrap();
	check(snapshot, history, &configs, &inventory, at, None)
}

#[test]
fn a_healthy_citation_reports_only_the_citing_project() {
	let mut repo = citing("version: 1\n", &up_guide(), &["up/guide.md"], &[]);
	let fresh = repo.evaluate(day(1), None);
	assert_clean(&fresh);
	assert_eq!(fresh.subjects.len(), 1, "{:#?}", fresh.subjects);
	assert_eq!(facts(document(&fresh, "docs/guide.md")), [Fact::Fresh]);
	assert_eq!(
		gates(&fresh),
		(ExitCode::BelowThreshold, ExitCode::BelowThreshold)
	);

	repo.commit(5, &[("up/code.rs", Some("v2"))]);
	let stale = repo.evaluate(day(6), None);
	assert_clean(&stale);
	assert_eq!(stale.subjects.len(), 1, "{:#?}", stale.subjects);
	assert_eq!(
		facts(document(&stale, "docs/guide.md")),
		[Fact::UpstreamStale, Fact::UpstreamDue]
	);
}

#[test]
fn a_hashing_error_on_a_cited_documents_source_is_reported() {
	let repo = citing("version: 1\n", &up_guide(), &["up/guide.md"], &[]);
	for canonical in [
		Canonical::CrlfInIndex,
		Canonical::UnsupportedLfsExtension,
		Canonical::UnsupportedFilter {
			filter: "custom".to_owned(),
			attribute_origin: ".gitattributes".to_owned(),
		},
	] {
		let snapshot = repo.checkout().with_canonical("up/code.rs", canonical);
		let evaluation = evaluate(&repo.history, &snapshot, day(1), None);
		let direct = check_project(&repo.history, &snapshot, "up", day(1)).unwrap();
		assert_eq!(evaluation.diagnostics, direct.diagnostics);
		assert_eq!(evaluation.diagnostics.len(), 1);
		assert_eq!(evaluation.diagnostics[0].location.file, path("up/code.rs"));
		assert!(!evaluation.is_complete());
		assert_eq!(
			gates(&evaluation),
			(ExitCode::Operational, ExitCode::Operational)
		);
	}
}

#[test]
fn a_hashing_error_on_an_unrelated_documents_source_is_not_reported() {
	let documents = json!({
		"guide.md": reviewed("/code.rs", "v1"),
		"other.md": reviewed("/other.rs", "o1"),
	});
	let repo = citing(
		"version: 1\n",
		&documents,
		&["up/guide.md"],
		&[("up/other.md", "# Other\n"), ("up/other.rs", "o1")],
	);
	let snapshot = repo
		.checkout()
		.with_canonical("up/other.rs", Canonical::CrlfInIndex);
	let direct = check_project(&repo.history, &snapshot, "up", day(1)).unwrap();
	assert_eq!(kinds(&direct), [DiagnosticKind::CrlfInIndex]);

	let evaluation = evaluate(&repo.history, &snapshot, day(1), None);
	assert_clean(&evaluation);
	assert_eq!(facts(document(&evaluation, "docs/guide.md")), [Fact::Fresh]);
	assert_eq!(
		gates(&evaluation),
		(ExitCode::BelowThreshold, ExitCode::BelowThreshold)
	);
}

#[test]
fn the_other_projects_invalid_time_zone_is_reported_once() {
	let documents = json!({
		"guide.md": reviewed("/code.rs", "v1"),
		"notes.md": reviewed("/code.rs", "v1"),
	});
	let repo = citing(
		"version: 1\ntime-zone: Not/AZone\n",
		&documents,
		&["up/guide.md", "up/notes.md"],
		&[("up/notes.md", "# Notes\n")],
	);
	let evaluation = repo.evaluate(day(1), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::InvalidTimeZone]);
	assert_eq!(evaluation.diagnostics[0].location.file, path(UP_CONFIG));
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}

/// A cadence with no occurrence within the supported range leaves the
/// documents under it without a deadline.
#[test]
fn an_inherited_schedule_without_a_deadline_is_reported_for_its_citation() {
	let config = "version: 1\nschedule:\n  grace: 5\n  cadence:\n    dtstart: \
	              2026-01-02\n    rrule: 'FREQ=YEARLY;INTERVAL=500'\n";
	let mut repo =
		citing(config, &json!({}), &["up/a.md"], &[("up/a.md", "# A\n")]);
	repo.commit(30, &[(UP_LOCK, Some(&lockfile(&json!({"a.md": {}}))))]);
	let snapshot = repo.checkout();
	let direct = check_project(&repo.history, &snapshot, "up", day(31)).unwrap();
	assert_eq!(kinds(&direct), [DiagnosticKind::InvalidSchedule]);
	assert_eq!(direct.diagnostics[0].location.file, path(UP_CONFIG));

	let evaluation = repo.evaluate(day(31), None);
	assert_eq!(evaluation.diagnostics, direct.diagnostics);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}

#[test]
fn a_backend_failure_in_an_unrelated_document_does_not_stop_the_check() {
	const BAD_LOCK: &str = "up/bad/source-sentry.lock";
	let first = lockfile(&json!({"bad.md": {"sources": [{"path": "code.rs"}]}}));
	let second = lockfile(&json!({"bad.md": {
		"schedule": {"grace": 3}, "sources": [{"path": "code.rs"}],
	}}));
	let mut repo = citing(
		"version: 1\n",
		&up_guide(),
		&["up/guide.md"],
		&[
			("up/bad/bad.md", "# Bad\n"),
			("up/bad/code.rs", "b1"),
			(BAD_LOCK, &first),
		],
	);
	repo.commit(1, &[(BAD_LOCK, Some(&second))]);
	repo.history.forget_blob(&fake_oid(first.as_bytes()));
	let snapshot = repo.checkout();
	let direct = check_project(&repo.history, &snapshot, "up", day(2));
	assert!(
		matches!(&direct, Err(error) if error.kind == BackendErrorKind::MissingObject),
		"{direct:#?}"
	);

	let evaluation = repo.evaluate(day(2), None);
	assert_clean(&evaluation);
	assert_eq!(facts(document(&evaluation, "docs/guide.md")), [Fact::Fresh]);
}

/// A living series whose older entry `up/notes/v1.md` awaits its seal, has
/// a stale source and cites the stale `up/base.md`; `up/other.md` is
/// unrelated.
#[test]
fn a_cited_documents_subject_matches_a_check_of_its_own_project() {
	let config = "version: 1\nschedule: {grace: 4}\nseries:\n  notes:\n    \
	              pattern: 'notes/v(?P<version>\\d+)\\.md'\n    version-kind: \
	              integer\n    lifecycle: living\n";
	let documents = json!({
		"base.md": reviewed("/lib.rs", "l1"),
		"other.md": reviewed("/other.rs", "o1"),
		"notes/v1.md": {"reviewed": review(), "sources": [
			attested("/code.rs", "v1"), attested("/base.md", "# Base\n"),
		]},
		"notes/v2.md": reviewed("/code.rs", "v1"),
	});
	let mut repo = citing(
		config,
		&documents,
		&["up/notes/v1.md"],
		&[
			("up/base.md", "# Base\n"),
			("up/lib.rs", "l1"),
			("up/other.md", "# Other\n"),
			("up/other.rs", "o1"),
			("up/notes/v1.md", "# V1\n"),
			("up/notes/v2.md", "# V2\n"),
		],
	);
	repo.commit(
		5,
		&[
			("up/code.rs", Some("v2")),
			("up/lib.rs", Some("l2")),
			("up/notes/v2.md", Some("# V2, revised\n")),
		],
	);
	let snapshot = repo.checkout();
	let configs = ConfigSet::load_project(&snapshot, &path("up")).unwrap();
	let found = discover(&snapshot, &configs).unwrap();
	let full = check_local(
		&snapshot,
		&repo.history,
		&configs,
		&found,
		day(6),
		Coverage::Project(None),
	)
	.unwrap();
	let cited: BTreeSet<DocumentId> = ["up/notes/v1.md", "up/base.md"]
		.into_iter()
		.map(|file| DocumentId::Local(path(file)))
		.collect();
	let restricted = check_local(
		&snapshot,
		&repo.history,
		&configs,
		&narrowed(&found, &cited),
		day(6),
		Coverage::Cited(&cited),
	)
	.unwrap();

	assert_clean(&restricted);
	assert_eq!(restricted.subjects.len(), 2, "{:#?}", restricted.subjects);
	assert_eq!(
		facts(document(&restricted, "up/notes/v1.md")),
		[Fact::Stale, Fact::UpstreamStale, Fact::AwaitingSeal]
	);
	for file in ["up/notes/v1.md", "up/base.md"] {
		let (whole, alone) = (document(&full, file), document(&restricted, file));
		assert_eq!(alone.kind, whole.kind);
		assert_eq!(alone.facts, whole.facts);
		assert_eq!(alone.deadline, whole.deadline);
		assert_eq!(alone.settings, whole.settings);
		assert!(alone.judgement.is_none());
	}

	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	assert_eq!(
		facts(document(&evaluation, "docs/guide.md")),
		[Fact::UpstreamStale]
	);
}

#[test]
fn the_other_projects_contexts_are_never_applied() {
	let failing = "version: 1\ndefine:\n  contexts:\n    default:\n      \
	               fallback: fail\n";
	let mut evaluations = Vec::new();
	for config in ["version: 1\n", failing] {
		let mut repo = citing(config, &up_guide(), &["up/guide.md"], &[]);
		repo.commit(5, &[("up/code.rs", Some("v2"))]);
		let direct =
			check_project(&repo.history, &repo.checkout(), "up", day(6)).unwrap();
		let expected = if config == failing {
			Outcome::Fail
		} else {
			Outcome::Warn
		};
		assert_eq!(outcome(document(&direct, "up/guide.md")), expected);
		evaluations.push(repo.evaluate(day(6), None));
	}
	let (plain, under_failing) = (&evaluations[0], &evaluations[1]);
	assert_clean(plain);
	assert_eq!(plain.subjects, under_failing.subjects);
	assert_eq!(plain.diagnostics, under_failing.diagnostics);
	assert_eq!(plain.summary, under_failing.summary);
}

#[test]
fn an_unreadable_declaration_of_a_cited_document_is_reported() {
	let unreadable =
		"---\nsource-sentry:\n  unknown-field: true\n---\n# Source\n";
	let repo = citing(
		"version: 1\n",
		&up_guide(),
		&["up/source.md"],
		&[("up/source.md", unreadable)],
	);
	let direct =
		check_project(&repo.history, &repo.checkout(), "up", day(1)).unwrap();
	assert!(
		kinds(&direct).contains(&DiagnosticKind::InvalidDeclaration),
		"{:#?}",
		direct.diagnostics
	);

	let evaluation = repo.evaluate(day(1), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::InvalidDeclaration]);
	assert_eq!(
		evaluation.diagnostics[0].location.file,
		path("up/source.md")
	);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);

	let unrelated = citing(
		"version: 1\n",
		&up_guide(),
		&["up/guide.md"],
		&[("up/source.md", unreadable)],
	);
	let evaluation = unrelated.evaluate(day(1), None);
	assert_clean(&evaluation);
	assert_eq!(facts(document(&evaluation, "docs/guide.md")), [Fact::Fresh]);
}

#[test]
fn a_duplicate_version_is_reported_whichever_member_is_cited() {
	let config = "version: 1\nseries:\n  audit:\n    pattern: \
	              'v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
	              lifecycle: records-only\n";
	let documents = json!({
		"guide.md": reviewed("/code.rs", "v1"),
		"v1.md": {},
	});
	let members = [("up/v01.md", "# V01\n"), ("up/v1.md", "# V1\n")];
	for cited in ["up/v1.md", "up/v01.md"] {
		let repo = citing(config, &documents, &[cited], &members);
		let evaluation = repo.evaluate(day(1), None);
		assert_eq!(
			kinds(&evaluation),
			[DiagnosticKind::DuplicateVersion],
			"{cited}"
		);
		assert_eq!(
			gates(&evaluation),
			(
				ExitCode::ConfigurationOrIntegrity,
				ExitCode::ConfigurationOrIntegrity
			),
			"{cited}"
		);
	}

	let unrelated = citing(config, &documents, &["up/guide.md"], &members);
	let evaluation = unrelated.evaluate(day(1), None);
	assert_clean(&evaluation);
}

#[test]
fn an_unreadable_lockfile_that_could_declare_a_cited_document_is_reported() {
	let truncated = r#"{"schema":1,"documents":{"source.md":{}},"retired":"#;
	for cited in ["up/sub/source.md", "up/sub/deep/source.md"] {
		let files = [
			("up/sub/source-sentry.lock", truncated),
			("up/sub/source.md", "# Source\n"),
			("up/sub/deep/source.md", "# Deep source\n"),
		];
		let repo = citing("version: 1\n", &up_guide(), &[cited], &files);
		let direct =
			check_project(&repo.history, &repo.checkout(), "up", day(1)).unwrap();
		assert_eq!(kinds(&direct), [DiagnosticKind::InvalidLockfile]);

		let evaluation = repo.evaluate(day(1), None);
		assert_eq!(
			kinds(&evaluation),
			[DiagnosticKind::InvalidLockfile],
			"{cited}"
		);
		assert_eq!(
			evaluation.diagnostics[0].location.file,
			path("up/sub/source-sentry.lock")
		);
		assert_eq!(
			gates(&evaluation),
			(
				ExitCode::ConfigurationOrIntegrity,
				ExitCode::ConfigurationOrIntegrity
			),
			"{cited}"
		);
	}

	// Neither the cited document's directory nor one above it.
	for cited in ["up/guide.md", "up/other/source.md"] {
		let files = [
			("up/sub/source-sentry.lock", truncated),
			("up/other/source.md", "# Other source\n"),
		];
		let repo = citing("version: 1\n", &up_guide(), &[cited], &files);
		assert_clean(&repo.evaluate(day(1), None));
	}
}
