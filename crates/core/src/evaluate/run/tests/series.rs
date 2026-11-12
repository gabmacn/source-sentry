//! Series: `awaiting-seal`, living heads and cadence.

use chrono::NaiveDate;
use serde_json::json;

use super::{
	CONFIG, REPOSITORY, Repo, assert_clean, day, document, evaluate, facts,
	gates, kinds, lockfile, outcome, review, seal,
};
use crate::claims::DiagnosticKind;
use crate::evaluate::run::{CompletionState, Evaluation, Subject};
use crate::model::{Fact, Outcome};
use crate::policy::ExitCode;
use crate::testing::path;

/// The audits' lockfile.
const AUDITS_LOCK: &str = "audits/source-sentry.lock";

/// The proposals' lockfile.
const PROPOSALS_LOCK: &str = "proposals/source-sentry.lock";

/// The repository config with a `records-only` series of audits, whose
/// schedule is `audits_schedule`, and a `living` series of proposals.
fn config(audits_schedule: &str) -> String {
	format!(
		"{CONFIG}series:\n  audits:\n    pattern: \
		 'audits/audit-(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
		 lifecycle: records-only\n{audits_schedule}  proposals:\n    pattern: \
		 'proposals/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
		 lifecycle: living\n"
	)
}

/// A monthly cadence from 1 January 2026, as the audits' schedule.
const MONTHLY: &str = "    schedule:\n      cadence:\n        dtstart: \
                       2026-01-01\n        rrule: 'FREQ=MONTHLY'\n";

/// The subject of the series named `name`.
fn series<'a>(evaluation: &'a Evaluation, name: &str) -> &'a Subject {
	evaluation
		.subjects
		.iter()
		.find(|subject| {
			subject
				.series()
				.is_some_and(|series| series.series.name == name)
		})
		.unwrap_or_else(|| panic!("no series {name}"))
}

/// In a `living` series the predecessor awaits its seal,
/// and in a `records-only` series every unsealed entry does.
#[test]
fn unsealed_entries_await_their_seal_in_both_lifecycles() {
	let repo = Repo::new(&[
		(REPOSITORY, &config("")),
		("audits/audit-1.md", "audit 1"),
		("audits/audit-2.md", "audit 2"),
		(
			AUDITS_LOCK,
			&lockfile(&json!({
				"audit-1.md": {"seal": seal("audit 1")},
				"audit-2.md": {"reviewed": review()},
			})),
		),
		("proposals/v1.md", "v1"),
		("proposals/v2.md", "v2"),
		(
			PROPOSALS_LOCK,
			&lockfile(&json!({
				"v1.md": {"reviewed": review()},
				"v2.md": {"reviewed": review()},
			})),
		),
	]);
	let evaluation = repo.evaluate(day(1), None);
	assert_clean(&evaluation);
	assert_eq!(
		facts(document(&evaluation, "audits/audit-1.md")),
		[Fact::Fresh]
	);
	let awaiting = document(&evaluation, "audits/audit-2.md");
	assert_eq!(facts(awaiting), [Fact::Fresh, Fact::AwaitingSeal]);
	assert_eq!(
		awaiting
			.document()
			.unwrap()
			.awaiting_seal
			.as_ref()
			.unwrap()
			.name,
		"audits"
	);
	assert_eq!(outcome(awaiting), Outcome::Warn);
	assert_eq!(
		facts(document(&evaluation, "proposals/v1.md")),
		[Fact::Fresh, Fact::AwaitingSeal]
	);
	assert_eq!(
		facts(document(&evaluation, "proposals/v2.md")),
		[Fact::Fresh]
	);
	let proposals = series(&evaluation, "proposals");
	assert_eq!(
		proposals.series().unwrap().head,
		Some(path("proposals/v2.md"))
	);
	assert!(proposals.facts.is_empty());
	assert_eq!(outcome(proposals), Outcome::Pass);
}

/// A `living` series whose newest entry is sealed is a
/// configuration error.
#[test]
fn a_sealed_living_head_is_a_configuration_error() {
	let repo = Repo::new(&[
		(REPOSITORY, &config("")),
		("proposals/v1.md", "v1"),
		("proposals/v2.md", "v2"),
		(
			PROPOSALS_LOCK,
			&lockfile(&json!({
				"v1.md": {"reviewed": review()},
				"v2.md": {"seal": seal("v2")},
			})),
		),
	]);
	let evaluation = repo.evaluate(day(1), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::SealedHead]);
	assert_eq!(
		evaluation.diagnostics[0].location.field.as_deref(),
		Some("series.proposals")
	);
	assert_eq!(
		gates(&evaluation),
		(
			ExitCode::ConfigurationOrIntegrity,
			ExitCode::ConfigurationOrIntegrity
		)
	);
}

/// A completion credits its window, a closed window without one is
/// missed, and the series is overdue while its newest closed window has no
/// credit. A seal that is not committed yet does not count.
#[test]
fn series_cadence_gives_overdue_and_missed() {
	let mut repo = Repo::new(&[
		(REPOSITORY, &config(MONTHLY)),
		("audits/audit-1.md", "audit 1"),
		(
			AUDITS_LOCK,
			&lockfile(&json!({"audit-1.md": {"reviewed": review()}})),
		),
	]);
	let sealed_one = repo.commit(
		10,
		&[(
			AUDITS_LOCK,
			Some(&lockfile(&json!({"audit-1.md": {"seal": seal("audit 1")}}))),
		)],
	);
	repo.commit(
		60,
		&[
			("audits/audit-2.md", Some("audit 2")),
			(
				AUDITS_LOCK,
				Some(&lockfile(&json!({
					"audit-1.md": {"seal": seal("audit 1")},
					"audit-2.md": {"reviewed": review()},
				}))),
			),
		],
	);
	let both_sealed = lockfile(&json!({
		"audit-1.md": {"seal": seal("audit 1")},
		"audit-2.md": {"seal": seal("audit 2")},
	}));

	// On 12 March, with the second seal only in the working tree.
	let snapshot = repo
		.checkout()
		.with_file(AUDITS_LOCK, both_sealed.as_bytes());
	let evaluation = evaluate(&repo.history, &snapshot, day(70), None);
	assert_clean(&evaluation);
	let audits = series(&evaluation, "audits");
	assert_eq!(facts(audits), [Fact::Overdue, Fact::Missed]);
	let cadence = audits.series().unwrap().cadence.as_ref().unwrap();
	let states: Vec<(String, &CompletionState)> = cadence
		.completions
		.iter()
		.map(|completion| (completion.member.to_string(), &completion.state))
		.collect();
	let CompletionState::Completed(first) = states[0].1 else {
		panic!("the first audit completed: {states:?}");
	};
	assert_eq!((&first.commit, first.timestamp), (&sealed_one, day(10)));
	assert_eq!(states[1].1, &CompletionState::Uncommitted);
	let windows = cadence.status.as_ref().unwrap();
	assert!(windows.overdue);
	assert_eq!(windows.missed, [0]);
	// The window that ended on 1 March is the one overdue.
	assert_eq!(
		audits.deadline.unwrap().due,
		NaiveDate::from_ymd_opt(2026, 3, 1)
	);
	assert_eq!(outcome(audits), Outcome::Warn);
	assert_eq!(
		gates(&evaluation),
		(ExitCode::Warning, ExitCode::BelowThreshold)
	);

	// Once the seal is committed, the overdue window becomes missed: a notice,
	// which never fails a gate.
	repo.commit(71, &[(AUDITS_LOCK, Some(&both_sealed))]);
	let evaluation = repo.evaluate(day(72), None);
	assert_clean(&evaluation);
	let audits = series(&evaluation, "audits");
	assert_eq!(facts(audits), [Fact::Missed]);
	assert_eq!(outcome(audits), Outcome::Notice);
	assert_eq!(
		gates(&evaluation),
		(ExitCode::BelowThreshold, ExitCode::BelowThreshold)
	);
}

#[test]
fn a_tombstones_completion_clock_error_points_at_its_marker_line() {
	for (lifecycle, marker, value) in [
		("records-only", "seal", seal("old entry")),
		("living", "completed", review()),
	] {
		let config = format!(
			"{CONFIG}series:\n  entries:\n    pattern: \
			 'entries/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
			 lifecycle: {lifecycle}\n{MONTHLY}"
		);
		let retired = review();
		let lock = format!(
			r#"{{
  "documents": {{}},
  "retired": {{
    "v1.md": [{{
      "retired": {retired},
      "{marker}": {value}
    }}]
  }},
  "schema": 1
}}"#
		);
		let repo = Repo::new(&[
			(REPOSITORY, &config),
			("entries/source-sentry.lock", &lock),
		]);
		// The completion's introducing commit exceeds the future-date tolerance.
		let evaluation = repo.evaluate(day(-30), None);
		let diagnostic = evaluation
			.diagnostics
			.iter()
			.find(|diagnostic| diagnostic.kind == DiagnosticKind::FutureDated)
			.unwrap_or_else(|| panic!("{lifecycle}: {evaluation:#?}"));
		assert_eq!(diagnostic.location.file, path("entries/source-sentry.lock"));
		assert_eq!(diagnostic.location.line, Some(6));
		assert_eq!(
			diagnostic.location.field,
			Some(format!("retired[\"v1.md\"][0].{marker}"))
		);
	}
}

/// Identical markers from successive documents at one path still date
/// separately, both while the replacement is live and after it is retired.
#[test]
fn every_incarnation_credits_its_own_window_even_with_identical_markers() {
	for (lifecycle, marker, value) in [
		("records-only", "seal", seal("entry")),
		("living", "completed", review()),
	] {
		let monthly = MONTHLY.replace("2026-01-01", "2026-02-01");
		let config = format!(
			"{CONFIG}series:\n  entries:\n    pattern: \
			 'entries/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
			 lifecycle: {lifecycle}\n{monthly}"
		);
		let retired = json!({"retired": review(), marker: value});
		let lock = |live: bool, tombstones: &[serde_json::Value]| {
			let mut documents = json!({"v2.md": {}});
			if live {
				documents["v1.md"] = json!({marker: value});
			}
			let retired = if tombstones.is_empty() {
				json!({})
			} else {
				json!({"v1.md": tombstones})
			};
			json!({"documents": documents, "retired": retired, "schema": 1})
				.to_string()
		};
		let file = "entries/source-sentry.lock";
		let mut repo = Repo::new(&[
			(REPOSITORY, &config),
			("entries/v2.md", "head"),
			(file, &lock(false, &[])),
		]);
		repo.commit(
			5,
			&[
				("entries/v1.md", Some("entry")),
				(file, Some(&lock(true, &[]))),
			],
		);
		repo.commit(
			6,
			&[
				("entries/v1.md", None),
				(file, Some(&lock(false, std::slice::from_ref(&retired)))),
			],
		);
		repo.commit(
			40,
			&[
				("entries/v1.md", Some("entry")),
				(file, Some(&lock(true, std::slice::from_ref(&retired)))),
			],
		);
		for stage in ["lockfile", "on-file", "retired"] {
			if stage == "on-file" {
				if lifecycle == "records-only" {
					continue;
				}
				let declaration = json!({marker: value});
				let on_file = format!("---\nsource-sentry: {declaration}\n---\nentry");
				repo.commit(
					41,
					&[
						("entries/v1.md", Some(&on_file)),
						(file, Some(&lock(false, std::slice::from_ref(&retired)))),
					],
				);
			}
			if stage == "retired" {
				repo.commit(
					42,
					&[
						("entries/v1.md", None),
						(
							file,
							Some(&lock(false, &[retired.clone(), retired.clone()])),
						),
					],
				);
			}
			let evaluation = repo.evaluate(day(60), None);
			assert_clean(&evaluation);
			let entries = series(&evaluation, "entries").series().unwrap();
			let cadence = entries.cadence.as_ref().unwrap();
			assert_eq!(cadence.completions.len(), 2);
			let status = cadence.status.as_ref().unwrap();
			assert_eq!(status.missed, [] as [usize; 0]);
			assert!(!status.overdue);
			for (index, expected) in [(0, day(5)), (1, day(40))] {
				let credit = status.windows[index].credit.as_ref().unwrap();
				assert_eq!(credit.entry, path("entries/v1.md"));
				assert_eq!(credit.at, expected);
			}
		}
	}
}
