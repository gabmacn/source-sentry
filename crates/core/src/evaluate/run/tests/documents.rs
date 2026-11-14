//! Live documents: drift, grace, cadence, timing, unattested declarations,
//! pattern membership, `upstream-stale` and remote documents.

use chrono::NaiveDate;
use serde_json::json;

use super::{
	CONFIG, DOCS_LOCK, REPOSITORY, Repo, assert_clean, attested, day, document,
	evaluate, facts, gates, lockfile, oid, outcome, review, started, timing,
};
use crate::claims::Mode;
use crate::config::Scope;
use crate::evaluate::run::{DocumentState, SourceState, Timing};
use crate::evaluate::schedule::Deadline;
use crate::evaluate::settings::SettingOrigin;
use crate::model::{DocumentId, Fact, Outcome};
use crate::policy::ExitCode;
use crate::testing::path;

/// A repository whose `docs/a.md` has the source `src/a.rs`, attested at
/// `v1`, with `entry` merged into its lockfile entry.
fn one_source(extra: &serde_json::Value) -> Repo {
	let mut entry = json!({"sources": [attested("/src/a.rs", "v1")]});
	if let (Some(entry), Some(extra)) = (entry.as_object_mut(), extra.as_object())
	{
		entry.extend(extra.clone());
	}
	Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		(DOCS_LOCK, &lockfile(&json!({"a.md": entry}))),
	])
}

#[test]
fn a_document_whose_sources_match_is_fresh_and_passes() {
	let repo = one_source(&json!({}));
	let evaluation = repo.evaluate(day(10), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Fresh]);
	assert_eq!(outcome(subject), Outcome::Pass);
	assert_eq!(subject.deadline, None);
	assert!(evaluation.is_complete());
	assert_eq!(
		gates(&evaluation),
		(ExitCode::BelowThreshold, ExitCode::BelowThreshold)
	);
}

/// Review evidence includes a pattern's own review and legacy member reviews.
#[test]
fn pattern_and_member_reviews_clear_unattested_independently() {
	for (source, expected) in [
		(json!({"pattern": "/src/*.rs"}), true),
		(json!({"pattern": "/src/*.rs", "reviewed": review()}), false),
		(
			json!({"pattern": "/src/*.rs", "members": [attested("/src/a.rs", "v1")]}),
			false,
		),
		(
			json!({"pattern": "/src/*.rs", "reviewed": review(), "members": [attested("/src/a.rs", "v1")]}),
			false,
		),
	] {
		let repo = Repo::new(&[
			(REPOSITORY, CONFIG),
			("src/a.rs", "v1"),
			("docs/a.md", "# A\n"),
			(
				DOCS_LOCK,
				&lockfile(&json!({"a.md": {
					"schedule": {"grace": 30}, "sources": [source],
				}})),
			),
		]);
		let evaluation = repo.evaluate(day(1), None);
		assert_clean(&evaluation);
		assert_eq!(
			facts(document(&evaluation, "docs/a.md")).contains(&Fact::Unattested),
			expected,
		);
	}
}

/// A pattern review never attests new matches, and its timestamp dates no clock.
#[test]
fn a_reviewed_empty_patterns_new_match_starts_at_the_files_commit() {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("docs/a.md", "# A\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {
				"schedule": {"grace": 3},
				"sources": [{"pattern": "/src/*.rs", "reviewed": {
					"at": "2099-01-01T00:00:00Z", "by": null,
				}}],
			}})),
		),
	]);
	let empty = repo.evaluate(day(1), None);
	let subject = document(&empty, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Fresh]);
	assert!(subject.deadline.is_none());
	let added = repo.commit(5, &[("src/a.rs", Some("new"))]);
	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	let (commit, timestamp) =
		started(timing(&evaluation, "docs/a.md", "src/a.rs"));
	assert_eq!((commit, timestamp), (&added, day(5)));
	assert_eq!(
		subject.deadline.unwrap().due,
		NaiveDate::from_ymd_opt(2026, 1, 9)
	);
}

/// The clock starts at the first change after the
/// attested oid, a later change does not reset it, and grace defers the
/// deadline to the end of its last day.
#[test]
fn drift_starts_at_the_first_change_and_grace_defers_the_deadline() {
	let mut repo = one_source(&json!({"schedule": {"grace": 3}}));
	let change = repo.commit(5, &[("src/a.rs", Some("v2"))]);
	repo.commit(7, &[("src/a.rs", Some("v3"))]);

	// Before the due day, the document is stale, and only a notice.
	let early = repo.evaluate(day(7), None);
	let subject = document(&early, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	assert_eq!(outcome(subject), Outcome::Notice);

	let evaluation = repo.evaluate(day(8), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due]);
	let (commit, timestamp) =
		started(timing(&evaluation, "docs/a.md", "src/a.rs"));
	assert_eq!((commit, timestamp), (&change, day(5)));
	let deadline = subject.deadline.unwrap();
	// Drift on 6 January plus three days: due on 9 January, from its first
	// instant, and overdue after its end.
	assert_eq!(deadline.due, NaiveDate::from_ymd_opt(2026, 1, 9));
	assert_eq!(deadline.due_from, day(8));
	assert_eq!(deadline.overdue_from, day(9));
	assert!(!deadline.provisional);
	assert_eq!(subject.settings.grace.value, 3);
	assert!(matches!(
		subject.settings.grace.origin,
		SettingOrigin::Declaration { .. }
	));
	assert_eq!(outcome(subject), Outcome::Warn);

	let overdue = repo.evaluate(day(9), None);
	assert_eq!(
		facts(document(&overdue, "docs/a.md")),
		[Fact::Stale, Fact::Due, Fact::Overdue]
	);
}

/// With a cadence, the deadline is the first cadence date
/// at least the grace period after the drift.
#[test]
fn a_cadence_moves_the_deadline_to_the_next_cadence_date() {
	let schedule = json!({"schedule": {
		"grace": 2,
		"cadence": {"dtstart": "2026-01-01", "rrule": "FREQ=MONTHLY"},
	}});
	let mut repo = one_source(&schedule);
	repo.commit(5, &[("src/a.rs", Some("v2"))]);

	let evaluation = repo.evaluate(day(20), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	let deadline = subject.deadline.unwrap();
	assert_eq!(deadline.due, NaiveDate::from_ymd_opt(2026, 2, 1));
	assert_eq!(deadline.overdue_from, day(32));

	let overdue = repo.evaluate(day(32), None);
	assert_eq!(
		facts(document(&overdue, "docs/a.md")),
		[Fact::Stale, Fact::Due, Fact::Overdue]
	);
}

/// An attested oid that never appears in the history
/// gives `timing-unknown`, `stale` and `overdue` at once, whatever the
/// grace period.
#[test]
fn an_attested_oid_missing_from_history_is_timing_unknown_and_overdue() {
	let repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {
				"schedule": {"grace": 30},
				"sources": [attested("/src/a.rs", "never committed")],
			}})),
		),
	]);
	let evaluation = repo.evaluate(day(1), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(
		facts(subject),
		[Fact::Stale, Fact::Due, Fact::Overdue, Fact::TimingUnknown]
	);
	assert_eq!(
		timing(&evaluation, "docs/a.md", "src/a.rs"),
		&Timing::Unknown
	);
	// A completed evaluation with unknown timing, not an incomplete one.
	assert!(evaluation.is_complete());
}

/// Drift that exists only in the working tree is stale, but its clock has
/// not started. A provisional clock at the instant of evaluation decides
/// `due`: without a schedule the document is due at once, but never
/// overdue before the change is committed.
#[test]
fn uncommitted_drift_is_stale_and_due_but_not_started() {
	let repo = one_source(&json!({}));
	let snapshot = repo.checkout().with_file("src/a.rs", b"edited");
	let evaluation = evaluate(&repo.history, &snapshot, day(10), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due]);
	let provisional = Deadline {
		due: None,
		due_from: day(10),
		overdue_from: day(10),
		provisional: true,
	};
	assert_eq!(subject.deadline, Some(provisional));
	assert_eq!(
		timing(&evaluation, "docs/a.md", "src/a.rs"),
		&Timing::NotStarted(Some(provisional))
	);
}

/// A document that was never reviewed is
/// `unattested`, also without sources, and its clock starts at the commit
/// that introduced its declaration, in a lockfile or in frontmatter.
#[test]
fn an_unattested_document_starts_its_clock_at_its_declaration() {
	let mut repo = Repo::new(&[(REPOSITORY, CONFIG), ("src/a.rs", "v1")]);
	repo.commit(
		1,
		&[("docs/a.md", Some("# A\n")), ("docs/b.md", Some("# B\n"))],
	);
	let declared = repo.commit(
		2,
		&[
			(
				DOCS_LOCK,
				Some(&lockfile(&json!({
					"a.md": {"schedule": {"grace": 5}},
					"b.md": {"sources": [{"path": "/src/a.rs"}]},
				}))),
			),
			(
				"docs/c.md",
				Some("---\nsource-sentry:\n  owners: ['docs']\n---\n# C\n"),
			),
		],
	);
	repo.commit(3, &[("src/a.rs", Some("v2"))]);

	let evaluation = repo.evaluate(day(4), None);
	assert_clean(&evaluation);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::Unattested]);
	let Some(DocumentState::Live(live)) =
		a.document().map(|document| &document.state)
	else {
		panic!("a live document");
	};
	let (commit, timestamp) = started(live.unattested.as_ref().unwrap());
	assert_eq!((commit, timestamp), (&declared, day(2)));
	// Declared on 3 January with five days of grace.
	assert_eq!(a.deadline.unwrap().due, NaiveDate::from_ymd_opt(2026, 1, 8));

	// No grace: overdue from the declaration's commit. Its unattested
	// source has no clock of its own, and the document is never `stale`.
	let b = document(&evaluation, "docs/b.md");
	assert_eq!(facts(b), [Fact::Due, Fact::Overdue, Fact::Unattested]);
	assert_eq!(timing(&evaluation, "docs/b.md", "src/a.rs"), &Timing::Idle);

	let c = document(&evaluation, "docs/c.md");
	assert_eq!(c.document().unwrap().mode, Mode::OnFile);
	assert_eq!(facts(c), [Fact::Due, Fact::Overdue, Fact::Unattested]);
	assert_eq!(c.deadline.unwrap().overdue_from, day(2));
}

/// An unattested declaration that is not committed yet has not started its
/// clock, so its deadline is provisional.
#[test]
fn an_uncommitted_declaration_is_unattested_with_a_provisional_deadline() {
	let repo = Repo::new(&[(REPOSITORY, CONFIG), ("docs/a.md", "# A\n")]);
	let snapshot = repo
		.checkout()
		.with_file(DOCS_LOCK, lockfile(&json!({"a.md": {}})).as_bytes());
	let evaluation = evaluate(&repo.history, &snapshot, day(3), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	// Without a schedule, the provisional clock makes it due at once.
	assert_eq!(facts(subject), [Fact::Due, Fact::Unattested]);
	assert!(
		subject
			.deadline
			.is_some_and(|deadline| deadline.provisional)
	);
}

/// A reviewed document with an unattested source is stale, and the source's
/// clock starts when it was declared.
#[test]
fn a_new_unattested_source_starts_when_it_was_declared() {
	let mut repo = one_source(&json!({"reviewed": review()}));
	let declared = repo.commit(
		4,
		&[(
			DOCS_LOCK,
			Some(&lockfile(&json!({"a.md": {
				"reviewed": review(),
				"sources": [attested("/src/a.rs", "v1"), {"path": "/src/b.rs"}],
			}}))),
		)],
	);
	let added = repo.commit(5, &[("src/b.rs", Some("b"))]);
	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due, Fact::Overdue]);
	// Declared on day 4; the file arrived on day 5, but the obligation is the
	// declaration's.
	let (commit, _) = started(timing(&evaluation, "docs/a.md", "src/b.rs"));
	assert_eq!(commit, &declared);
	assert_ne!(commit, &added);
}

/// A new match is an unattested source whose clock starts
/// where the pattern was declared and the file existed; a removed member is
/// a change whose clock starts at the deleting commit.
#[test]
fn pattern_additions_and_removals_are_changes_with_their_own_clocks() {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/m/one.rs", "one"),
		("src/m/two.rs", "two"),
		("src/m/four.rs", "four"),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"a.md": {
					"schedule": {"grace": 1},
					"sources": [{"pattern": "/src/m/{one,two,three}.rs", "members": [
						{"path": "/src/m/one.rs", "oid": oid("one"), "reviewed": review()},
						{"path": "/src/m/two.rs", "oid": oid("two"), "reviewed": review()},
					]}],
				},
				"b.md": {"reviewed": review()},
			})),
		),
	]);
	let added = repo.commit(3, &[("src/m/three.rs", Some("three"))]);
	let pattern = repo.commit(
		4,
		&[(
			DOCS_LOCK,
			Some(&lockfile(&json!({
				"a.md": {
					"schedule": {"grace": 1},
					"sources": [{"pattern": "/src/m/{one,two,three}.rs", "members": [
						{"path": "/src/m/one.rs", "oid": oid("one"), "reviewed": review()},
						{"path": "/src/m/two.rs", "oid": oid("two"), "reviewed": review()},
					]}],
				},
				"b.md": {"reviewed": review(), "sources": [{"pattern": "/src/m/f*.rs"}]},
			}))),
		)],
	);
	let removed = repo.commit(5, &[("src/m/two.rs", None)]);

	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::Stale, Fact::Due, Fact::Overdue]);
	let Some(DocumentState::Live(live)) =
		a.document().map(|document| &document.state)
	else {
		panic!("a live document");
	};
	let states: Vec<(String, SourceState)> = live
		.sources
		.iter()
		.map(|source| (source.path.to_string(), source.state))
		.collect();
	assert_eq!(
		states,
		[
			("src/m/one.rs".to_owned(), SourceState::Fresh),
			("src/m/three.rs".to_owned(), SourceState::Added),
			("src/m/two.rs".to_owned(), SourceState::Removed),
		]
	);
	assert_eq!(
		started(timing(&evaluation, "docs/a.md", "src/m/three.rs")).0,
		&added
	);
	assert_eq!(
		started(timing(&evaluation, "docs/a.md", "src/m/two.rs")).0,
		&removed
	);
	// The earliest deadline: the addition on 4 January, plus one day.
	assert_eq!(a.deadline.unwrap().overdue_from, day(5));

	// `four.rs` existed from the start, but the pattern arrived on day 4.
	let b = document(&evaluation, "docs/b.md");
	assert_eq!(facts(b), [Fact::Stale, Fact::Due, Fact::Overdue]);
	assert_eq!(
		started(timing(&evaluation, "docs/b.md", "src/m/four.rs")).0,
		&pattern
	);
}

/// A repository whose `docs/a.md` has the pattern source `/src/m/*.rs`,
/// with `one.rs` and `two.rs` attested.
fn two_members() -> Repo {
	Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/m/one.rs", "one"),
		("src/m/two.rs", "two"),
		(
			"docs/a.md",
			"# A
",
		),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {
				"schedule": {"grace": 3},
				"sources": [{"pattern": "/src/m/*.rs", "members": [
					{"path": "/src/m/one.rs", "oid": oid("one"), "reviewed": review()},
					{"path": "/src/m/two.rs", "oid": oid("two"), "reviewed": review()},
				]}],
			}})),
		),
	])
}

/// A member that matched its attestation until it was
/// deleted starts its clock at the deleting commit.
#[test]
fn a_member_fresh_until_deleted_starts_at_the_deletion() {
	let mut repo = two_members();
	let deletion = repo.commit(4, &[("src/m/two.rs", None)]);
	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	let (commit, timestamp) =
		started(timing(&evaluation, "docs/a.md", "src/m/two.rs"));
	assert_eq!((commit, timestamp), (&deletion, day(4)));
	// Deleted on 5 January with three days of grace.
	assert_eq!(
		subject.deadline.unwrap().due,
		NaiveDate::from_ymd_opt(2026, 1, 8)
	);
}

/// A deletion is a later change, so a member that drifted before
/// it was deleted keeps the clock of that earlier drift.
#[test]
fn a_member_that_drifted_before_deletion_keeps_its_earlier_clock() {
	let mut repo = two_members();
	let drift = repo.commit(1, &[("src/m/two.rs", Some("two, edited"))]);
	repo.commit(4, &[("src/m/two.rs", None)]);
	let evaluation = repo.evaluate(day(6), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	// Drifted on 2 January with three days of grace: overdue from 6 January.
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due, Fact::Overdue]);
	let (commit, timestamp) =
		started(timing(&evaluation, "docs/a.md", "src/m/two.rs"));
	assert_eq!((commit, timestamp), (&drift, day(1)));
	assert_eq!(subject.deadline.unwrap().overdue_from, day(5));
}

/// A document whose upstream document is stale carries
/// `upstream-stale`, transitively.
#[test]
fn upstream_staleness_propagates_through_document_sources() {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		("docs/c.md", "# C\n"),
		("docs/d.md", "# D\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"a.md": {"sources": [attested("/src/a.rs", "v1")]},
				"b.md": {"sources": [attested("/docs/a.md", "# A\n")]},
				"c.md": {"sources": [attested("/docs/b.md", "# B\n")]},
				"d.md": {"sources": [attested("/src/a.rs", "v1")]},
			})),
		),
	]);
	let fresh = repo.evaluate(day(1), None);
	assert_clean(&fresh);
	for file in ["docs/a.md", "docs/b.md", "docs/c.md"] {
		assert_eq!(facts(document(&fresh, file)), [Fact::Fresh], "{file}");
	}

	repo.commit(2, &[("src/a.rs", Some("v2"))]);
	let evaluation = repo.evaluate(day(3), None);
	assert_clean(&evaluation);
	assert_eq!(
		facts(document(&evaluation, "docs/a.md")),
		[Fact::Stale, Fact::Due, Fact::Overdue]
	);
	for (file, upstream) in
		[("docs/b.md", "docs/a.md"), ("docs/c.md", "docs/b.md")]
	{
		let subject = document(&evaluation, file);
		assert_eq!(
			facts(subject),
			[Fact::UpstreamStale, Fact::UpstreamDue],
			"{file}"
		);
		let Some(DocumentState::Live(live)) =
			subject.document().map(|document| &document.state)
		else {
			panic!("a live document");
		};
		assert_eq!(live.stale_upstreams, [path(upstream)]);
		assert_eq!(live.due_upstreams, [path(upstream)]);
	}
}

/// A dependant shares its upstream document's treatment: while a scheduled
/// upstream document is only stale, its dependants are `upstream-stale`
/// and a notice; once it is due, they are `upstream-due` and warn,
/// transitively.
#[test]
fn a_dependant_is_due_when_its_upstream_document_is() {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		("docs/c.md", "# C\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"a.md": {
					"sources": [attested("/src/a.rs", "v1")],
					"schedule": {"grace": 3},
				},
				"b.md": {"sources": [attested("/docs/a.md", "# A\n")]},
				"c.md": {"sources": [attested("/docs/b.md", "# B\n")]},
			})),
		),
	]);
	// Drift on 3 January with three days of grace: due on 6 January.
	repo.commit(2, &[("src/a.rs", Some("v2"))]);

	let early = repo.evaluate(day(4), None);
	assert_clean(&early);
	assert_eq!(facts(document(&early, "docs/a.md")), [Fact::Stale]);
	for file in ["docs/b.md", "docs/c.md"] {
		assert_eq!(
			facts(document(&early, file)),
			[Fact::UpstreamStale],
			"{file}"
		);
	}
	for file in ["docs/a.md", "docs/b.md", "docs/c.md"] {
		assert_eq!(outcome(document(&early, file)), Outcome::Notice, "{file}");
	}

	let due = repo.evaluate(day(5), None);
	assert_clean(&due);
	assert_eq!(facts(document(&due, "docs/a.md")), [Fact::Stale, Fact::Due]);
	for (file, upstream) in
		[("docs/b.md", "docs/a.md"), ("docs/c.md", "docs/b.md")]
	{
		let subject = document(&due, file);
		assert_eq!(
			facts(subject),
			[Fact::UpstreamStale, Fact::UpstreamDue],
			"{file}"
		);
		assert_eq!(outcome(subject), Outcome::Warn, "{file}");
		let Some(DocumentState::Live(live)) =
			subject.document().map(|document| &document.state)
		else {
			panic!("a live document");
		};
		assert_eq!(live.due_upstreams, [path(upstream)]);
	}
}

/// An uncommitted change to a scheduled document takes a provisional clock
/// at the instant of evaluation: it is not due before its grace ends, and
/// its provisional deadline says when it would be.
#[test]
fn uncommitted_drift_with_grace_has_a_provisional_deadline() {
	let repo = one_source(&json!({"schedule": {"grace": 3}}));
	let snapshot = repo.checkout().with_file("src/a.rs", b"edited");
	let evaluation = evaluate(&repo.history, &snapshot, day(10), None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale]);
	assert_eq!(outcome(subject), Outcome::Notice);
	let deadline = subject.deadline.unwrap();
	assert!(deadline.provisional);
	// 11 January plus three days.
	assert_eq!(deadline.due, NaiveDate::from_ymd_opt(2026, 1, 14));
	assert_eq!(deadline.due_from, day(13));
}

/// A clock dated after the instant of evaluation, within the tolerance
/// that lets it be used, is due from that instant: the change has landed,
/// even if a clock says it lands later. When it is overdue does not change.
#[test]
fn a_clock_dated_in_the_future_is_due_at_the_instant_of_evaluation() {
	let mut repo = one_source(&json!({}));
	repo.commit(5, &[("src/a.rs", Some("v2"))]);
	// Twelve hours before the commit's date.
	let at = day(5) - chrono::Duration::hours(12);
	let evaluation = repo.evaluate(at, None);
	assert_clean(&evaluation);
	let subject = document(&evaluation, "docs/a.md");
	assert_eq!(facts(subject), [Fact::Stale, Fact::Due]);
	assert_eq!(outcome(subject), Outcome::Warn);
	let deadline = subject.deadline.unwrap();
	assert_eq!(deadline.due_from, at);
	assert_eq!(deadline.overdue_from, day(5));
}

/// A remote document takes its paired config's defaults, and its
/// sources drift like any other.
#[test]
fn a_remote_document_uses_its_owners_defaults() {
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("src/a.rs", "v1"),
		(
			".source-sentry/remotes/legal.yaml",
			"classes: ['privacy']\nowners: ['legal']\nschedule:\n  grace: 14\n",
		),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&json!({"abstract:privacy-video": {
				"sources": [attested("/src/a.rs", "v1")],
			}})),
		),
	]);
	repo.commit(2, &[("src/a.rs", Some("v2"))]);

	let evaluation = repo.evaluate(day(3), Some("production"));
	assert_clean(&evaluation);
	let id = DocumentId::Remote("abstract:privacy-video".parse().unwrap());
	let subject = evaluation.subjects_of(&id).next().unwrap();
	assert_eq!(facts(subject), [Fact::Stale]);
	assert_eq!(subject.settings.grace.value, 14);
	let SettingOrigin::Config(origin) = &subject.settings.classes.origin else {
		panic!("classes from the paired config");
	};
	assert_eq!(origin.scope, Scope::Remote("legal".to_owned()));
	assert_eq!(subject.settings.owners.value, ["legal"]);
	// A privacy document, stale in production, fails.
	assert_eq!(outcome(subject), Outcome::Fail);
	assert_eq!(gates(&evaluation), (ExitCode::Failure, ExitCode::Failure));
}
