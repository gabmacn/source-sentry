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
