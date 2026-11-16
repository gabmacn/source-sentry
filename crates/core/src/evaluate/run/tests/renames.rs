//! A renamed document keeps the clocks of the documents that cite it: their
//! drift is read at the path the source had at each commit.

use chrono::NaiveDate;
use serde_json::{Value, json};

use super::{
	CONFIG, DOCS_LOCK, REPOSITORY, Repo, attested, day, document, evaluate,
	facts, lockfile, review, started, timing,
};
use crate::evaluate::run::Timing;
use crate::model::Fact;

/// A rename marker from the project-rooted `from`.
fn renamed(from: &str, n: i64) -> Value {
	json!({"from": from, "at": day(n).to_rfc3339(), "by": null})
}

/// The docs lockfile: the tracked source document `source`, renamed through
/// `markers`, and `docs/d.md`, which cites it with a week's grace through
/// `cited`, a source entry.
fn docs_lock(source: &str, markers: &[Value], cited: &Value) -> String {
	let mut entry = json!({"reviewed": review()});
	if !markers.is_empty() {
		entry["renamed"] = json!(markers);
	}
	lockfile(&json!({
		source: entry,
		"d.md": {"schedule": {"grace": 7}, "sources": [cited]},
	}))
}

/// A repository where `docs/d.md` attests `docs/s.md` at `v1` on day 0, and
/// `docs/s.md` changes on day 2, with `extra` files from the start. Returns
/// the commit of the change.
fn cited_and_changed(extra: &[(&str, &str)]) -> (Repo, crate::ports::CommitId) {
	let lock = docs_lock("s.md", &[], &attested("/docs/s.md", "v1"));
	let mut files = vec![
		(REPOSITORY, CONFIG),
		("docs/s.md", "v1"),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, lock.as_str()),
	];
	files.extend_from_slice(extra);
	let mut repo = Repo::new(&files);
	let change = repo.commit(2, &[("docs/s.md", Some("v2"))]);
	(repo, change)
}

/// The changes that rename `docs/<old>` to `docs/<new>`, whose content is
/// `v2`, as `rename` records it.
fn rename_changes<'a>(
	old: &'a str,
	new: &'a str,
	lock: &'a str,
) -> Vec<(&'a str, Option<&'a str>)> {
	vec![(old, None), (new, Some("v2")), (DOCS_LOCK, Some(lock))]
}

/// The started clock, deadline and facts of `docs/d.md`, for its source at
/// `source`.
fn clock_of(
	evaluation: &crate::evaluate::run::Evaluation,
	source: &str,
) -> (
	Timing,
	Option<crate::evaluate::schedule::Deadline>,
	Vec<Fact>,
) {
	let subject = document(evaluation, "docs/d.md");
	(
		timing(evaluation, "docs/d.md", source).clone(),
		subject.deadline,
		facts(subject),
	)
}

/// A document that cites a renamed source keeps the clock its source's
/// change started, with the same deadline and facts, before and after the
/// rename is committed.
#[test]
fn a_renamed_source_keeps_the_citing_documents_clock() {
	let (mut repo, change) = cited_and_changed(&[]);
	let before = repo.evaluate(day(5), None);
	let (timing_before, deadline_before, facts_before) =
		clock_of(&before, "docs/s.md");
	assert_eq!(started(&timing_before), (&change, day(2)));
	assert_eq!(facts_before, [Fact::Stale]);

	let lock = docs_lock(
		"r.md",
		&[renamed("/docs/s.md", 4)],
		&attested("/docs/r.md", "v1"),
	);
	let uncommitted = repo
		.checkout()
		.without_file("docs/s.md")
		.with_file("docs/r.md", b"v2")
		.with_file(DOCS_LOCK, lock.as_bytes());
	let pending = evaluate(&repo.history, &uncommitted, day(5), None);
	repo.commit(4, &rename_changes("docs/s.md", "docs/r.md", &lock));
	let committed = repo.evaluate(day(5), None);
	for after in [&pending, &committed] {
		let (timing_after, deadline_after, facts_after) =
			clock_of(after, "docs/r.md");
		assert_eq!(started(&timing_after), (&change, day(2)));
		assert_eq!(deadline_after, deadline_before);
		assert_eq!(facts_after, facts_before);
	}
}

/// A rename to a path that once held the attested content does not move the
/// clock to the rename: the change at the source's former path still dates
/// it, so the document stays overdue.
#[test]
fn a_rename_onto_a_path_with_the_attested_content_keeps_the_clock() {
	let (mut repo, change) = cited_and_changed(&[("docs/r.md", "v1")]);
	let before = repo.evaluate(day(10), None);
	let (_, deadline_before, facts_before) = clock_of(&before, "docs/s.md");
	assert_eq!(facts_before, [Fact::Stale, Fact::Due, Fact::Overdue]);
	assert_eq!(
		deadline_before.and_then(|deadline| deadline.due),
		NaiveDate::from_ymd_opt(2026, 1, 10)
	);

	let lock = docs_lock(
		"r.md",
		&[renamed("/docs/s.md", 4)],
		&attested("/docs/r.md", "v1"),
	);
	repo.commit(4, &rename_changes("docs/s.md", "docs/r.md", &lock));
	let after = repo.evaluate(day(10), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&after, "docs/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// Two renames in a row are followed back to the first path.
#[test]
fn a_source_renamed_twice_keeps_the_citing_documents_clock() {
	let (mut repo, change) = cited_and_changed(&[]);
	let before = repo.evaluate(day(8), None);
	let (_, deadline_before, facts_before) = clock_of(&before, "docs/s.md");

	let first = renamed("/docs/s.md", 4);
	let lock = docs_lock(
		"q.md",
		std::slice::from_ref(&first),
		&attested("/docs/q.md", "v1"),
	);
	repo.commit(4, &rename_changes("docs/s.md", "docs/q.md", &lock));
	let lock = docs_lock(
		"r.md",
		&[first, renamed("/docs/q.md", 6)],
		&attested("/docs/r.md", "v1"),
	);
	repo.commit(6, &rename_changes("docs/q.md", "docs/r.md", &lock));

	let after = repo.evaluate(day(8), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&after, "docs/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// `lock` with its tombstones replaced by one for each of `retired`.
fn with_tombstones(lock: &str, retired: &[&str]) -> String {
	let mut lock: Value = serde_json::from_str(lock).unwrap();
	for key in retired {
		lock["retired"][key] = json!([{"retired": review()}]);
	}
	serde_json::to_string_pretty(&lock).unwrap()
}

/// The docs lockfile in which `docs/d.md` cites `cited` with a week's grace,
/// and `source`, a document that is no longer tracked, has a tombstone.
fn retired_lock(source: &str, cited: &Value) -> String {
	with_tombstones(
		&lockfile(&json!({
			"d.md": {"schedule": {"grace": 7}, "sources": [cited]},
		})),
		&[source],
	)
}

/// A document that cites a renamed source keeps the clock its source's
/// change started, with the same deadline and facts, once that source is
/// retired and no longer tracked, its file left in place.
#[test]
fn a_renamed_then_retired_source_keeps_the_citing_documents_clock() {
	let (mut repo, change) = cited_and_changed(&[]);
	let lock = docs_lock(
		"r.md",
		&[renamed("/docs/s.md", 4)],
		&attested("/docs/r.md", "v1"),
	);
	repo.commit(4, &rename_changes("docs/s.md", "docs/r.md", &lock));
	let renamed = repo.evaluate(day(6), None);
	let (timing_before, deadline_before, facts_before) =
		clock_of(&renamed, "docs/r.md");
	assert_eq!(started(&timing_before), (&change, day(2)));
	assert_eq!(facts_before, [Fact::Stale]);
	assert!(deadline_before.is_some());

	let lock = retired_lock("r.md", &attested("/docs/r.md", "v1"));
	repo.commit(5, &[(DOCS_LOCK, Some(&lock))]);
	let retired = repo.evaluate(day(6), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&retired, "docs/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// A retired source that was never renamed keeps the citing document's
/// clock, as it did before its retirement.
#[test]
fn a_retired_source_never_renamed_keeps_the_citing_documents_clock() {
	let (mut repo, change) = cited_and_changed(&[]);
	let before = repo.evaluate(day(6), None);
	let (_, deadline_before, facts_before) = clock_of(&before, "docs/s.md");

	let lock = retired_lock("s.md", &attested("/docs/s.md", "v1"));
	repo.commit(5, &[(DOCS_LOCK, Some(&lock))]);
	let after = repo.evaluate(day(6), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&after, "docs/s.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// A document whose cited source is unchanged, but stale through its own
/// source, is upstream-stale; once that source document is retired, it is
/// no longer tracked, so the citing document is fresh.
#[test]
fn a_retired_source_document_no_longer_makes_its_citers_upstream_stale() {
	let lock = |retired: bool| {
		let mut documents = json!({
			"d.md": {"schedule": {"grace": 7}, "sources": [attested("/docs/s.md", "v1")]},
		});
		if retired {
			with_tombstones(&lockfile(&documents), &["s.md"])
		} else {
			documents["s.md"] = json!({
				"reviewed": review(),
				"sources": [attested("/docs/code.rs", "c1")],
			});
			lockfile(&documents)
		}
	};
	let tracked = lock(false);
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("docs/code.rs", "c1"),
		("docs/s.md", "v1"),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, tracked.as_str()),
	]);
	repo.commit(2, &[("docs/code.rs", Some("c2"))]);
	let before = repo.evaluate(day(6), None);
	let facts_before = facts(document(&before, "docs/d.md"));
	assert!(
		facts_before.contains(&Fact::UpstreamStale),
		"{facts_before:?}"
	);

	repo.commit(5, &[(DOCS_LOCK, Some(&lock(true)))]);
	let after = repo.evaluate(day(6), None);
	assert_eq!(facts(document(&after, "docs/d.md")), [Fact::Fresh]);
	assert_eq!(timing(&after, "docs/d.md", "docs/s.md"), &Timing::Idle);
}

/// A renamed document that is an attested member of a pattern source keeps
/// the member's clock.
#[test]
fn a_renamed_pattern_member_keeps_its_clock() {
	let member = |file: &str| json!({"pattern": "/docs/notes/*.md", "members": [attested(file, "v1")]});
	let lock = |source: &str, markers: &[Value], file: &str| {
		let mut entry = json!({"reviewed": review()});
		if !markers.is_empty() {
			entry["renamed"] = json!(markers);
		}
		lockfile(&json!({
			source: entry,
			"d.md": {"schedule": {"grace": 7}, "sources": [member(file)]},
		}))
	};
	let start = lock("notes/s.md", &[], "/docs/notes/s.md");
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("docs/notes/s.md", "v1"),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, &start),
	]);
	let change = repo.commit(2, &[("docs/notes/s.md", Some("v2"))]);
	let before = repo.evaluate(day(5), None);
	let (_, deadline_before, facts_before) = clock_of(&before, "docs/notes/s.md");

	let moved = lock(
		"notes/r.md",
		&[renamed("/docs/notes/s.md", 4)],
		"/docs/notes/r.md",
	);
	repo.commit(
		4,
		&rename_changes("docs/notes/s.md", "docs/notes/r.md", &moved),
	);
	let after = repo.evaluate(day(5), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&after, "docs/notes/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// Where the source's declaration cannot be read, the path it had is
/// uncertain, and the clock starts no later than it would have: content
/// that matches the attestation at the new path, before the rename, never
/// stops the search there.
#[test]
fn an_unreadable_declaration_never_delays_a_renamed_sources_clock() {
	let lock = docs_lock("s.md", &[], &attested("/docs/s.md", "v1"));
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		("docs/s.md", "v1"),
		("docs/r.md", "v1"),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, &lock),
	]);
	let change = repo.commit(
		2,
		&[
			("docs/s.md", Some("v2")),
			(DOCS_LOCK, Some("{not a lockfile")),
		],
	);
	let renamed_lock = docs_lock(
		"r.md",
		&[renamed("/docs/s.md", 4)],
		&attested("/docs/r.md", "v1"),
	);
	repo.commit(4, &rename_changes("docs/s.md", "docs/r.md", &renamed_lock));
	let evaluation = repo.evaluate(day(10), None);
	let (timing, _, _) = clock_of(&evaluation, "docs/r.md");
	assert_eq!(started(&timing), (&change, day(2)));
}

/// The config of the nested project `up`.
const UP_CONFIG: &str = "up/.source-sentry/source-sentry.yaml";

/// The top lockfile of `up`.
const UP_LOCK: &str = "up/source-sentry.lock";

/// The lockfile of `up`: the tracked document `source`, renamed through
/// `markers`.
fn up_lock(source: &str, markers: &[Value]) -> String {
	let mut entry = json!({"reviewed": review()});
	if !markers.is_empty() {
		entry["renamed"] = json!(markers);
	}
	lockfile(&json!({ source: entry }))
}

/// The docs lockfile of the root project: `docs/d.md` cites `cited`, a
/// source entry, with a week's grace.
fn citing_lock(cited: &Value) -> String {
	lockfile(&json!({
		"d.md": {"schedule": {"grace": 7}, "sources": [cited]},
	}))
}

/// A repository where `docs/d.md`, in the root project, attests
/// `up/s.md`, a document of the project `up`, at `v1` on day 0, and
/// `up/s.md` changes on day 2, with `extra` files from the start. Returns
/// the commit of the change.
fn cited_in_another_project(
	extra: &[(&str, &str)],
) -> (Repo, crate::ports::CommitId) {
	let up = up_lock("s.md", &[]);
	let docs = citing_lock(&attested("/up/s.md", "v1"));
	let mut files = vec![
		(REPOSITORY, CONFIG),
		(UP_CONFIG, "version: 1\n"),
		("up/s.md", "v1"),
		(UP_LOCK, up.as_str()),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, docs.as_str()),
	];
	files.extend_from_slice(extra);
	let mut repo = Repo::new(&files);
	let change = repo.commit(2, &[("up/s.md", Some("v2"))]);
	(repo, change)
}

/// The changes that rename `up/s.md` to `up/r.md`, whose content is `v2`,
/// as `rename` records it with `from`, and update the citation in
/// `docs/d.md`.
fn rename_in_up(from: &str) -> (String, String) {
	(
		up_lock("r.md", &[renamed(from, 4)]),
		citing_lock(&attested("/up/r.md", "v1")),
	)
}

/// A document that cites a document of another project keeps its clock,
/// deadline and facts when that document is renamed within its project,
/// before and after the rename is committed.
#[test]
fn a_source_renamed_in_another_project_keeps_the_citing_documents_clock() {
	let (mut repo, change) = cited_in_another_project(&[]);
	let before = repo.evaluate(day(5), None);
	let (timing_before, deadline_before, facts_before) =
		clock_of(&before, "up/s.md");
	assert_eq!(started(&timing_before), (&change, day(2)));
	assert_eq!(facts_before, [Fact::Stale]);
	assert!(deadline_before.is_some());

	let (up, docs) = rename_in_up("/s.md");
	let uncommitted = repo
		.checkout()
		.without_file("up/s.md")
		.with_file("up/r.md", b"v2")
		.with_file(UP_LOCK, up.as_bytes())
		.with_file(DOCS_LOCK, docs.as_bytes());
	let pending = evaluate(&repo.history, &uncommitted, day(5), None);
	repo.commit(
		4,
		&[
			("up/s.md", None),
			("up/r.md", Some("v2")),
			(UP_LOCK, Some(&up)),
			(DOCS_LOCK, Some(&docs)),
		],
	);
	let committed = repo.evaluate(day(5), None);
	for after in [&pending, &committed] {
		let (timing_after, deadline_after, facts_after) =
			clock_of(after, "up/r.md");
		assert_eq!(started(&timing_after), (&change, day(2)));
		assert_eq!(deadline_after, deadline_before);
		assert_eq!(facts_after, facts_before);
	}
}

/// A document that cites a document of another project keeps its clock,
/// deadline and facts when that document is renamed within its project
/// and then retired there.
#[test]
fn a_source_renamed_then_retired_in_another_project_keeps_the_clock() {
	let (mut repo, change) = cited_in_another_project(&[]);
	let (up, docs) = rename_in_up("/s.md");
	repo.commit(
		4,
		&[
			("up/s.md", None),
			("up/r.md", Some("v2")),
			(UP_LOCK, Some(&up)),
			(DOCS_LOCK, Some(&docs)),
		],
	);
	let renamed = repo.evaluate(day(6), None);
	let (timing_before, deadline_before, facts_before) =
		clock_of(&renamed, "up/r.md");
	assert_eq!(started(&timing_before), (&change, day(2)));
	assert!(deadline_before.is_some());

	let up = with_tombstones(&lockfile(&json!({})), &["r.md"]);
	repo.commit(5, &[(UP_LOCK, Some(&up))]);
	let retired = repo.evaluate(day(6), None);
	let (timing_after, deadline_after, facts_after) =
		clock_of(&retired, "up/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// A rename in another project, recorded with a repository-rooted `from`,
/// to a path that once held the attested content, does not move the clock
/// of the citing document to the rename: it stays overdue.
#[test]
fn a_rename_in_another_project_onto_the_attested_content_keeps_the_clock() {
	let (mut repo, change) = cited_in_another_project(&[("up/r.md", "v1")]);
	let before = repo.evaluate(day(10), None);
	let (_, deadline_before, facts_before) = clock_of(&before, "up/s.md");
	assert_eq!(facts_before, [Fact::Stale, Fact::Due, Fact::Overdue]);
	assert_eq!(
		deadline_before.and_then(|deadline| deadline.due),
		NaiveDate::from_ymd_opt(2026, 1, 10)
	);

	let (up, docs) = rename_in_up(":/up/s.md");
	repo.commit(
		4,
		&[
			("up/s.md", None),
			("up/r.md", Some("v2")),
			(UP_LOCK, Some(&up)),
			(DOCS_LOCK, Some(&docs)),
		],
	);
	let after = repo.evaluate(day(10), None);
	let (timing_after, deadline_after, facts_after) = clock_of(&after, "up/r.md");
	assert_eq!(started(&timing_after), (&change, day(2)));
	assert_eq!(deadline_after, deadline_before);
	assert_eq!(facts_after, facts_before);
}

/// A document of another project, cited by `docs/d.md`, keeps its clock
/// when its own source, in the citing project, is renamed onto a path that
/// once held the attested content: it stays overdue, so `docs/d.md` stays
/// `upstream-due`.
#[test]
fn a_cited_documents_source_renamed_in_the_citing_project_keeps_its_clock() {
	let up = |source: &str| {
		lockfile(&json!({"s.md": {
			"schedule": {"grace": 7},
			"reviewed": review(),
			"sources": [attested(source, "v1")],
		}}))
	};
	let docs = |source: &str, markers: &[Value]| {
		let mut entry = json!({"reviewed": review()});
		if !markers.is_empty() {
			entry["renamed"] = json!(markers);
		}
		lockfile(&json!({
			source: entry,
			"d.md": {"sources": [attested("/up/s.md", "# S\n")]},
		}))
	};
	let (up_start, docs_start) = (up(":/docs/t.md"), docs("t.md", &[]));
	let mut repo = Repo::new(&[
		(REPOSITORY, CONFIG),
		(UP_CONFIG, "version: 1\n"),
		("up/s.md", "# S\n"),
		(UP_LOCK, &up_start),
		("docs/t.md", "v1"),
		("docs/u.md", "v1"),
		("docs/d.md", "# D\n"),
		(DOCS_LOCK, &docs_start),
	]);
	repo.commit(2, &[("docs/t.md", Some("v2"))]);
	let before = repo.evaluate(day(10), None);
	let facts_before = facts(document(&before, "docs/d.md"));
	assert!(
		facts_before.contains(&Fact::UpstreamDue),
		"{facts_before:?}"
	);

	let (up_moved, docs_moved) =
		(up(":/docs/u.md"), docs("u.md", &[renamed("/docs/t.md", 4)]));
	repo.commit(
		4,
		&[
			("docs/t.md", None),
			("docs/u.md", Some("v2")),
			(DOCS_LOCK, Some(&docs_moved)),
			(UP_LOCK, Some(&up_moved)),
		],
	);
	let after = repo.evaluate(day(10), None);
	assert_eq!(facts(document(&after, "docs/d.md")), facts_before);
}
