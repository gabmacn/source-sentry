//! Contexts, unclaimed files, errors and exit codes.

use serde_json::json;

use super::{
	CONFIG, DOCS_LOCK, REPOSITORY, Repo, assert_clean, attested, day, document,
	evaluate, facts, gates, kinds, lockfile, outcome, timing,
};
use crate::claims::{DiagnosticKind, Severity};
use crate::evaluate::run::{RuleOrigin, SubjectKind, Timing};
use crate::evaluate::settings::SettingOrigin;
use crate::model::{Fact, Outcome};
use crate::policy::{Basis, ContextOrigin, ExitCode, RuleRef, Threshold};
use crate::ports::Canonical;

/// A repository whose privacy policy, `docs/privacy.md`, has a week of grace
/// and a source that changed on day 2.
fn privacy_policy() -> Repo {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/privacy.md", "# Privacy\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"privacy.md": {
				"classes": ["privacy"],
				"schedule": {"grace": 7},
				"sources": [attested("/src/a.rs", "v1")],
			}})),
		),
	]);
	repo.commit(2, &[("src/a.rs", Some("v2"))]);
	repo
}

/// In production, a stale privacy policy fails both gates during its grace
/// period; the built-in default context gives it a notice until it is due,
/// and warns from its due day.
#[test]
fn a_stale_privacy_policy_fails_in_production_and_is_a_notice_by_default() {
	let repo = privacy_policy();

	let production = repo.evaluate(day(3), Some("production"));
	assert_clean(&production);
	let subject = document(&production, "docs/privacy.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	assert_eq!(outcome(subject), Outcome::Fail);
	let judgement = subject.judgement.as_ref().unwrap();
	assert_eq!(judgement.facts[0].basis, Basis::Rule(RuleRef::Context(1)));
	let rule = production.rule(subject, RuleRef::Context(1)).unwrap();
	assert_eq!(rule.rule.outcome, Outcome::Fail);
	let RuleOrigin::Context { name, origin, .. } = rule.origin else {
		panic!("a context rule");
	};
	assert_eq!(
		(name.as_str(), origin),
		("production", ContextOrigin::Repository)
	);
	assert_eq!(gates(&production), (ExitCode::Failure, ExitCode::Failure));

	let default = repo.evaluate(day(3), None);
	assert_clean(&default);
	let subject = document(&default, "docs/privacy.md");
	assert_eq!(outcome(subject), Outcome::Notice);
	assert_eq!(
		default.context.as_ref().unwrap().selected.origin,
		ContextOrigin::BuiltIn { version: 1 }
	);
	assert_eq!(
		gates(&default),
		(ExitCode::BelowThreshold, ExitCode::BelowThreshold)
	);

	// Drift on 3 January plus seven days: due on 10 January.
	let due = repo.evaluate(day(9), None);
	assert_clean(&due);
	let subject = document(&due, "docs/privacy.md");
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due]);
	assert_eq!(outcome(subject), Outcome::Warn);
	assert_eq!(gates(&due), (ExitCode::Warning, ExitCode::BelowThreshold));
}

/// An unclaimed file is judged with the classes its
/// directory inherits from its root's config.
#[test]
fn an_unclaimed_file_is_judged_with_its_roots_classes() {
	let repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("docs/source-sentry.yaml", "classes: ['privacy']\n"),
		(DOCS_LOCK, &lockfile(&json!({}))),
		("docs/legal/stray.md", "# Stray\n"),
	]);
	let evaluation = repo.evaluate(day(1), Some("production"));
	assert_clean(&evaluation);
	let subject = evaluation
		.subjects
		.iter()
		.find(|subject| matches!(subject.kind, SubjectKind::Unclaimed(_)))
		.unwrap();
	assert_eq!(facts(subject), [Fact::Unclaimed]);
	assert_eq!(subject.settings.classes.value[0].as_str(), "privacy");
	assert!(matches!(
		subject.settings.classes.origin,
		SettingOrigin::Config(_)
	));
	assert_eq!(outcome(subject), Outcome::Fail);

	let default = repo.evaluate(day(1), None);
	assert_eq!(
		gates(&default),
		(ExitCode::Warning, ExitCode::BelowThreshold)
	);
}

/// An unknown context is a configuration error, and no
/// subject is judged.
#[test]
fn an_unknown_context_is_a_configuration_error() {
	let repo = privacy_policy();
	let evaluation = repo.evaluate(day(3), Some("prodution"));
	assert_eq!(kinds(&evaluation), [DiagnosticKind::UnknownContext]);
	assert!(evaluation.context.is_none());
	assert!(
		evaluation
			.subjects
			.iter()
			.all(|subject| subject.judgement.is_none())
	);
	// Facts do not depend on the context.
	assert_eq!(
		facts(document(&evaluation, "docs/privacy.md")),
		[Fact::Stale]
	);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}

/// A source behind an unsupported filter is left out of
/// the facts, and the evaluation is incomplete: exit 4.
#[test]
fn an_unsupported_filter_is_an_operational_error() {
	let repo = privacy_policy();
	let snapshot = repo.checkout().with_canonical(
		"src/a.rs",
		Canonical::UnsupportedFilter {
			filter: "crypt".to_owned(),
			attribute_origin: ".gitattributes:3".to_owned(),
		},
	);
	let evaluation = evaluate(&repo.history, &snapshot, day(3), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::UnsupportedFilter]);
	let diagnostic = &evaluation.diagnostics[0];
	assert_eq!(diagnostic.severity(), Severity::Operational);
	assert!(
		diagnostic
			.message
			.contains("unsupported clean filter crypt")
	);
	assert_eq!(
		facts(document(&evaluation, "docs/privacy.md")),
		[Fact::Fresh]
	);
	assert!(!evaluation.is_complete());
	assert_eq!(
		gates(&evaluation),
		(ExitCode::Operational, ExitCode::Operational)
	);
}

/// A submodule source is a configuration error at the
/// declaration that names it.
#[test]
fn a_submodule_source_is_a_configuration_error() {
	let repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("vendor/lib", "Subproject commit"),
		("docs/a.md", "# A\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {
				"sources": [attested("/vendor/lib", "Subproject commit")],
			}})),
		),
	]);
	let snapshot = repo
		.checkout()
		.with_canonical("vendor/lib", Canonical::Submodule);
	let evaluation = evaluate(&repo.history, &snapshot, day(1), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::Submodule]);
	let diagnostic = &evaluation.diagnostics[0];
	assert_eq!(diagnostic.location.file.as_str(), DOCS_LOCK);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("documents[\"a.md\"].sources[0].path")
	);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}

/// A drift search that reaches a shallow boundary is exit 4.
#[test]
fn a_shallow_history_is_an_operational_error() {
	let mut repo = privacy_policy();
	let head = repo.commit(3, &[("src/a.rs", Some("v3"))]);
	repo.history.make_shallow_at(&head);
	let evaluation = repo.evaluate(day(4), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::ShallowHistory]);
	assert!(evaluation.diagnostics[0].message.contains("--unshallow"));
	assert_eq!(
		timing(&evaluation, "docs/privacy.md", "src/a.rs"),
		&Timing::Failed
	);
	// Still stale, but without a deadline.
	let subject = document(&evaluation, "docs/privacy.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	assert_eq!(subject.deadline, None);
	assert_eq!(
		gates(&evaluation),
		(ExitCode::Operational, ExitCode::Operational)
	);
}

/// A clock dated more than a day after the evaluation instant is
/// an operational error, never replaced by the instant.
#[test]
fn a_future_dated_clock_is_an_operational_error() {
	let repo = privacy_policy();
	let evaluation = repo.evaluate(day(0), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::FutureDated]);
	assert_eq!(
		timing(&evaluation, "docs/privacy.md", "src/a.rs"),
		&Timing::Failed
	);
	assert_eq!(
		gates(&evaluation),
		(ExitCode::Operational, ExitCode::Operational)
	);
}

/// A time zone that is not an IANA name is a
/// configuration error, and deadlines cannot be computed.
#[test]
fn an_invalid_time_zone_is_a_configuration_error() {
	let mut repo = privacy_policy();
	repo.commit(
		3,
		&[(
			REPOSITORY,
			Some(&format!("{CONFIG}time-zone: 'Mars/Olympus'\n")),
		)],
	);
	let evaluation = repo.evaluate(day(4), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::InvalidTimeZone]);
	assert_eq!(document(&evaluation, "docs/privacy.md").deadline, None);
	assert_eq!(
		evaluation.exit_code(Threshold::Fail),
		ExitCode::ConfigurationOrIntegrity
	);
}

/// An invalid cadence is a configuration error at the
/// declaration that sets it.
#[test]
fn an_invalid_cadence_is_a_configuration_error() {
	let repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {
				"schedule": {"cadence": {"dtstart": "2026-01-02", "rrule": "FREQ=MONTHLY;BYMONTHDAY=1"}},
				"sources": [attested("/src/a.rs", "v1")],
			}})),
		),
	]);
	let evaluation = repo.evaluate(day(1), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::InvalidSchedule]);
	assert_eq!(
		evaluation.diagnostics[0].location.field.as_deref(),
		Some("documents[\"a.md\"].schedule.cadence")
	);
}

/// Each threshold's exit code, from the most severe outcome or
/// error. Errors outrank outcomes, and operational errors outrank the rest.
#[test]
fn each_threshold_gives_its_exit_code() {
	let fresh = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		(
			"docs/a.md",
			"# A
",
		),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {"sources": [attested("/src/a.rs", "v1")]}})),
		),
	])
	.evaluate(day(1), None);
	let repo = privacy_policy();
	let warned = repo.evaluate(day(9), None);
	let failed = repo.evaluate(day(3), Some("production"));
	let configuration = repo.evaluate(day(3), Some("unknown"));
	let snapshot = repo
		.checkout()
		.with_canonical("src/a.rs", Canonical::CrlfInIndex);
	let operational = evaluate(&repo.history, &snapshot, day(3), Some("unknown"));

	let codes: Vec<(u8, u8)> =
		[&fresh, &warned, &failed, &configuration, &operational]
			.iter()
			.map(|evaluation| {
				(
					evaluation.exit_code(Threshold::Warn).code(),
					evaluation.exit_code(Threshold::Fail).code(),
				)
			})
			.collect();
	assert_eq!(codes, [(0, 0), (1, 0), (2, 2), (3, 3), (4, 4)]);
}
