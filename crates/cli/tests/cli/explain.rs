//! `explain`.

use crate::support::{AT, TestRepo};

#[test]
fn explain_shows_settings_origins_facts_rules_and_the_deadline() {
	let repo = TestRepo::init();
	repo.write("docs/source-sentry.yaml", "schedule:\n  grace: 7\n");
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	let run = repo.run(&["explain", "docs/guide.md", "--at", AT]);
	run.expect_code(0);
	let text = &run.stdout;
	for expected in [
		"docs/guide.md",
		"Context default · 2026-11-07 18:00 UTC · working tree on",
		"tracked by its lockfile in docs/source-sentry.lock `documents[\"guide.md\"]`",
		"Facts     overdue (missed review by 2026-10-27)",
		"Deadline  review by 2026-10-27: due from 2026-10-27 00:00 UTC, overdue from 2026-10-28 00:00 UTC",
		"grace    7 days",
		"docs/source-sentry.yaml:2 (for directory:docs)",
		"src/a.rs  drifted",
		"counted from 2026-10-20 10:00 UTC",
		"no rule matches, so the fallback of default applies",
	] {
		assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
	}
}

#[test]
fn explain_finds_unclaimed_files_and_reports_unknown_subjects() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("docs/new-page.md", "# New\n");
	repo.git(&["add", "docs/new-page.md"]);

	let run = repo.run_in("docs", &["explain", "new-page.md", "--at", AT]);
	run.expect_code(0);
	assert!(
		run
			.stdout
			.contains("unclaimed, found by the scan of root .")
	);
	assert!(run.stdout.contains("unclaimed"), "{}", run.stdout);

	let unknown = repo.run(&["explain", "no-such-thing", "--at", AT]);
	unknown.expect_code(2);
	assert!(
		unknown.stderr.contains("nothing named"),
		"{}",
		unknown.stderr
	);
}

#[test]
fn explain_shows_and_exits_with_a_clock_that_cannot_be_dated() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-11-03T12:00:00Z");

	// More than 24 hours before the change commit.
	let at = "2026-11-02T00:00:00Z";
	repo.run(&["check", "--at", at]).expect_code(4);
	let run = repo.run(&["explain", "docs/guide.md", "--at", at]);
	run.expect_code(4);
	assert!(run.stdout.contains("Diagnostics"), "{}", run.stdout);
	assert!(run.stdout.contains("future-dated"), "{}", run.stdout);
}
