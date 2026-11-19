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

/// A document overdue only through a source whose timing is unknown does
/// not call the other source's deadline missed, and still shows that
/// deadline in full.
#[test]
fn explain_shows_a_future_deadline_beside_unknown_timing() {
	let repo = TestRepo::init();
	repo.guide_with_unknown_timing();

	let run =
		repo.run(&["explain", "docs/guide.md", "--at", "2026-11-16T12:00:00Z"]);
	run.expect_code(0);
	let text = &run.stdout;
	let deadline = "review by 2026-12-15: due from 2026-12-15 00:00 UTC, \
	                overdue from 2026-12-16 00:00 UTC";
	for expected in [
		"Facts     overdue, timing unknown\n".to_owned(),
		format!("Deadline  {deadline}\n"),
		format!("counted from 2026-11-15 12:00 UTC; {deadline}\n"),
		"changed, at an unknown time: the attested oid is not in the history"
			.to_owned(),
	] {
		assert!(text.contains(&expected), "missing {expected:?} in:\n{text}");
	}
	assert!(!text.contains("missed"), "{text}");
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

/// A context the project does not declare stops every subject in it being
/// judged: `explain` shows that error under a document, an unclaimed file
/// and a series, and exits 3 as `check` does.
#[test]
fn explain_shows_and_exits_with_an_unknown_context() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		r"version: 1
series:
  versions:
    pattern: 'docs/v(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: records-only
",
	);
	repo.write("docs/v1.md", "# Version\n");
	repo.reviewed_guide();
	repo.write("docs/new-page.md", "# New\n");
	repo.git(&["add", "docs/new-page.md"]);
	repo
		.run(&["check", "--at", AT, "--context", "prodution"])
		.expect_code(3);
	let subjects = ["docs/guide.md", "docs/new-page.md", "versions"];

	for subject in subjects {
		let run =
			repo.run(&["explain", subject, "--at", AT, "--context", "prodution"]);
		run.expect_code(3);
		let text = &run.stdout;
		let (_, diagnostics) = text
			.split_once("\nDiagnostics\n")
			.unwrap_or_else(|| panic!("no diagnostics for {subject} in:\n{text}"));
		assert!(
			diagnostics.contains(
				"unknown-context  .source-sentry/source-sentry.yaml \
				 `define.contexts`  context `prodution` is not declared"
			),
			"{subject}:\n{text}"
		);
	}

	// Under the default context, the same subjects carry no diagnostics.
	for subject in subjects {
		let run = repo.run(&["explain", subject, "--at", AT]);
		run.expect_code(0);
		assert!(!run.stdout.contains("Diagnostics"), "{}", run.stdout);
	}
}
