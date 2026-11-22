//! Reread schedules: when a whole document is due to be reread, from what
//! the count starts, and which documents they apply to.

use jiff::civil::Date;
use serde_json::{Value, json};

use super::{
	DOCS_LOCK, REPOSITORY, Repo, assert_clean, attested, day, document, evaluate,
	facts, kinds, lockfile, seal,
};
use crate::claims::DiagnosticKind;
use crate::evaluate::run::{DocumentState, RereadBase, Subject, Timing};
use crate::evaluate::schedule::{Deadline, DeadlineKind};
use crate::model::{DocumentId, Fact};

/// A project config whose documents are due to be reread on the first of
/// every month, from 1 January 2026.
const MONTHLY: &str = "version: 1\nschedule:\n  reread:\n    dtstart: \
                       2026-01-01\n    rrule: 'FREQ=MONTHLY'\n";

/// A whole-document reread at `at`, in JSON.
fn reread(at: &str) -> Value {
	json!({"at": at, "by": "QA"})
}

/// The reread deadline due on `due`, a day of 2026 in UTC.
fn reread_by(month: i8, day: i8) -> Deadline {
	let due = Date::new(2026, month, day).unwrap();
	let after = due.tomorrow().unwrap();
	Deadline {
		due: Some(due),
		due_from: due.to_zoned(jiff::tz::TimeZone::UTC).unwrap().timestamp(),
		overdue_from: after.to_zoned(jiff::tz::TimeZone::UTC).unwrap().timestamp(),
		provisional: false,
		kind: DeadlineKind::Reread,
	}
}

/// The reread base of a live document.
fn base(subject: &Subject) -> &RereadBase {
	let Some(DocumentState::Live(live)) =
		subject.document().map(|document| &document.state)
	else {
		panic!("not a live document: {subject:#?}");
	};
	&live.reread.as_ref().expect("a reread schedule").base
}

/// A document reread on 5 August is due again on 1 September, and overdue
/// from the day after; one reread on 1 August, a date of the schedule, is
/// due on 1 September too, not the same day.
#[test]
fn a_reread_is_due_on_the_first_schedule_date_after_it() {
	let repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"a.md": {"reviewed": reread("2026-08-05T10:00:00Z")},
				"b.md": {"reviewed": reread("2026-08-01T00:00:00Z")},
			})),
		),
	]);

	let before = repo.evaluate("2026-08-31T23:59:59Z".parse().unwrap(), None);
	assert_clean(&before);
	for file in ["docs/a.md", "docs/b.md"] {
		let subject = document(&before, file);
		assert_eq!(facts(subject), [Fact::Fresh]);
		assert_eq!(subject.deadline, Some(reread_by(9, 1)));
	}

	let due = repo.evaluate("2026-09-01T00:00:00Z".parse().unwrap(), None);
	assert_eq!(facts(document(&due, "docs/a.md")), [Fact::RereadDue]);
	assert_eq!(facts(document(&due, "docs/b.md")), [Fact::RereadDue]);

	let overdue = repo.evaluate("2026-09-02T00:00:00Z".parse().unwrap(), None);
	assert_eq!(
		facts(document(&overdue, "docs/a.md")),
		[Fact::RereadDue, Fact::RereadOverdue]
	);
	assert!(matches!(
		base(document(&overdue, "docs/a.md")),
		RereadBase::Reread(review) if review.by.as_deref() == Some("QA")
	));
}

/// A document never reread counts from the commit that introduced its
/// declaration, whether its sources were attested or nothing was ever
/// reviewed. A `dtstart` after that is itself the first due date.
#[test]
fn a_document_never_reread_counts_from_its_declaration() {
	let mut repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
	]);
	repo.commit(
		40,
		&[
			(
				".source-sentry/presets/later.yaml",
				Some(
					"schedule:\n  reread:\n    dtstart: 2026-06-01\n    rrule: \
					 'FREQ=MONTHLY'\n",
				),
			),
			("docs/c.md", Some("# C\n")),
			("later/d.md", Some("# D\n")),
			("later/source-sentry.yaml", Some("imports: [later]\n")),
			(
				"later/source-sentry.lock",
				Some(&lockfile(
					&json!({"d.md": {"sources": [attested("/src/a.rs", "v1")]}}),
				)),
			),
			(
				DOCS_LOCK,
				Some(&lockfile(&json!({
					"a.md": {"sources": [attested("/src/a.rs", "v1")]},
					"b.md": {},
				}))),
			),
		],
	);

	// Declared on 10 February: due on 1 March.
	let evaluation = repo.evaluate(day(70), None);
	assert_clean(&evaluation);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::RereadDue, Fact::RereadOverdue]);
	assert_eq!(a.deadline, Some(reread_by(3, 1)));
	assert!(matches!(
		base(a),
		RereadBase::Declared(Timing::Started(clock))
			if clock.start.timestamp == day(40)
			&& clock.deadline == Some(reread_by(3, 1))
	));
	// Never reviewed at all: unattested, and its reread is overdue too.
	assert_eq!(
		facts(document(&evaluation, "docs/b.md")),
		[
			Fact::Due,
			Fact::Overdue,
			Fact::RereadDue,
			Fact::RereadOverdue,
			Fact::Unattested
		]
	);

	// The preset's schedule starts on 1 June, which is its first due date.
	let d = document(&evaluation, "later/d.md");
	assert_eq!(facts(d), [Fact::Fresh]);
	assert_eq!(d.deadline, Some(reread_by(6, 1)));
}

/// A declaration that exists only in the working tree counts from the
/// instant of evaluation, provisionally: it is never `reread-overdue`.
#[test]
fn an_uncommitted_declaration_counts_provisionally() {
	let repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
	]);
	let snapshot = repo.checkout().with_file(
		DOCS_LOCK,
		lockfile(&json!({"a.md": {"sources": [attested("/src/a.rs", "v1")]}}))
			.as_bytes(),
	);

	let evaluation = evaluate(&repo.history, &snapshot, day(70), None);
	assert_clean(&evaluation);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::Fresh]);
	let provisional = Deadline {
		provisional: true,
		..reread_by(4, 1)
	};
	assert_eq!(a.deadline, Some(provisional));
	assert_eq!(
		base(a),
		&RereadBase::Declared(Timing::NotStarted(Some(provisional)))
	);
}

/// Only a reread resets the count: a review of the sources alone, later
/// than the last reread, leaves the deadline where it was. The reported
/// deadline is the earliest of a change's and the reread's.
#[test]
fn a_review_of_the_sources_does_not_reset_the_reread() {
	let mut repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("src/a.rs", "v1"),
		("docs/a.md", "# A\n"),
	]);
	repo.commit(
		1,
		&[(
			DOCS_LOCK,
			Some(&lockfile(&json!({"a.md": {
				"reviewed": reread("2026-01-01T12:00:00Z"),
				"sources": [{
					"path": "/src/a.rs",
					"oid": super::oid("v1"),
					"reviewed": reread("2026-01-20T12:00:00Z"),
				}],
			}}))),
		)],
	);

	let evaluation = repo.evaluate(day(40), None);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::RereadDue, Fact::RereadOverdue]);
	assert_eq!(a.deadline, Some(reread_by(2, 1)));

	// A change due at once on 11 February, after the reread deadline: the reread's
	// is reported, and the change's facts stay its own.
	repo.commit(41, &[("src/a.rs", Some("v2"))]);
	let evaluation = repo.evaluate(day(41), None);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(
		facts(a),
		[
			Fact::Stale,
			Fact::Due,
			Fact::Overdue,
			Fact::RereadDue,
			Fact::RereadOverdue
		]
	);
	assert_eq!(
		a.deadline.map(|deadline| deadline.kind),
		Some(DeadlineKind::Reread)
	);
}

/// The reread's date is read in the project's time zone: a reread at 13:00
/// UTC on 31 August is 1 September in Auckland, so it is due on 1 October.
#[test]
fn the_reread_date_is_local_to_the_project() {
	let repo = Repo::new(&[
		(
			REPOSITORY,
			&format!("{MONTHLY}time-zone: Pacific/Auckland\n"),
		),
		("docs/a.md", "# A\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"a.md": {"reviewed": reread("2026-08-31T13:00:00Z")}})),
		),
	]);

	let evaluation = repo.evaluate("2026-09-20T00:00:00Z".parse().unwrap(), None);
	assert_clean(&evaluation);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::Fresh]);
	let deadline = a.deadline.unwrap();
	assert_eq!(deadline.due, Some(Date::new(2026, 10, 1).unwrap()));
	// Midnight in Auckland, in daylight saving time, is 11:00 UTC.
	assert_eq!(deadline.due_from, "2026-09-30T11:00:00Z".parse().unwrap());
}

/// A sealed record, and a series entry awaiting its seal, are not subject
/// to the reread schedule; the living entry of a series is.
#[test]
fn records_and_entries_awaiting_their_seal_are_not_reread() {
	let config = format!(
		"{MONTHLY}series:\n  proposals:\n    pattern: \
		 'proposals/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
		 lifecycle: living\n"
	);
	let repo = Repo::new(&[
		(REPOSITORY, &config),
		("docs/record.md", "record"),
		(
			DOCS_LOCK,
			&lockfile(&json!({"record.md": {"seal": seal("record")}})),
		),
		("proposals/v1.md", "v1"),
		("proposals/v2.md", "v2"),
		(
			"proposals/source-sentry.lock",
			&lockfile(&json!({
				"v1.md": {"reviewed": reread("2026-01-01T00:00:00Z")},
				"v2.md": {"reviewed": reread("2026-01-01T00:00:00Z")},
			})),
		),
	]);

	let evaluation = repo.evaluate(day(100), None);
	assert_clean(&evaluation);
	let record = document(&evaluation, "docs/record.md");
	assert_eq!(facts(record), [Fact::Fresh]);
	assert_eq!(record.deadline, None);
	let awaiting = document(&evaluation, "proposals/v1.md");
	assert_eq!(facts(awaiting), [Fact::Fresh, Fact::AwaitingSeal]);
	assert_eq!(awaiting.deadline, None);
	assert_eq!(
		facts(document(&evaluation, "proposals/v2.md")),
		[Fact::RereadDue, Fact::RereadOverdue]
	);
}

/// A due reread says nothing about a change in content, so a document that
/// cites one carries neither `upstream-due` nor `upstream-stale`.
#[test]
fn a_citing_document_gains_nothing_from_a_due_reread() {
	let repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("docs/host.md", "# Host\n"),
		("docs/guide.md", "# Guide\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"host.md": {"reviewed": reread("2026-01-01T00:00:00Z")},
				"guide.md": {
					"reviewed": reread("2026-03-15T00:00:00Z"),
					"schedule": {"reread": null},
					"sources": [attested("/docs/host.md", "# Host\n")],
				},
			})),
		),
	]);

	let evaluation = repo.evaluate(day(80), None);
	assert_clean(&evaluation);
	assert_eq!(
		facts(document(&evaluation, "docs/host.md")),
		[Fact::RereadDue, Fact::RereadOverdue]
	);
	let guide = document(&evaluation, "docs/guide.md");
	assert_eq!(facts(guide), [Fact::Fresh]);
	assert_eq!(guide.deadline, None);
}

/// A reread dated after the instant of evaluation would postpone the next
/// one, so it is a configuration error at the `reviewed` field, with no
/// reread deadline. Without a reread schedule it is not checked.
#[test]
fn a_reread_dated_in_the_future_is_a_configuration_error() {
	let repo = Repo::new(&[
		(REPOSITORY, MONTHLY),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		(
			DOCS_LOCK,
			&lockfile(&json!({
				"a.md": {"reviewed": reread("2026-12-01T00:00:00Z")},
				"b.md": {
					"reviewed": reread("2026-12-01T00:00:00Z"),
					"schedule": {"reread": null},
				},
			})),
		),
	]);

	let evaluation = repo.evaluate(day(100), None);
	assert_eq!(kinds(&evaluation), [DiagnosticKind::FutureReread]);
	let diagnostic = &evaluation.diagnostics[0];
	assert_eq!(diagnostic.location.file.as_str(), DOCS_LOCK);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("documents[\"a.md\"].reviewed")
	);
	let a = document(&evaluation, "docs/a.md");
	assert_eq!(facts(a), [Fact::Fresh]);
	assert_eq!(a.deadline, None);
}

/// A remote document follows the reread schedule of its owner's config,
/// counted from its declaration in the owner lockfile when it was never
/// reread.
#[test]
fn a_remote_document_follows_its_owners_reread_schedule() {
	let mut repo = Repo::new(&[
		(REPOSITORY, "version: 1\n"),
		("src/a.rs", "v1"),
		(
			".source-sentry/remotes/legal.yaml",
			"schedule:\n  reread:\n    dtstart: 2026-01-01\n    rrule: 'FREQ=MONTHLY'\n",
		),
	]);
	repo.commit(
		40,
		&[(
			".source-sentry/remotes/legal.lock",
			Some(&lockfile(&json!({
				"abstract:video": {"sources": [attested("/src/a.rs", "v1")]},
			}))),
		)],
	);

	let evaluation = repo.evaluate(day(70), None);
	assert_clean(&evaluation);
	let video = DocumentId::Remote("abstract:video".parse().unwrap());
	let subject = evaluation.subjects_of(&video).next().unwrap();
	assert_eq!(facts(subject), [Fact::RereadDue, Fact::RereadOverdue]);
	assert_eq!(subject.deadline, Some(reread_by(3, 1)));
}
