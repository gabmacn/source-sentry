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

/// Under `--quiet`, `explain` still prints the errors that set its exit
/// code, as `check` does, and nothing else.
#[test]
fn quiet_explain_prints_its_errors() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["rm", "--quiet", "src/a.rs"]);
	repo.commit_at("2026-10-20T10:00:00Z");

	let run = repo.run(&["--quiet", "explain", "docs/guide.md", "--at", AT]);
	run.expect_code(3);
	assert_eq!(run.stdout, "");
	assert!(run.stderr.contains("missing-source"), "{}", run.stderr);
	assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
}

/// A source that cannot be hashed is reported once by `check`, at the
/// file; `explain` shows that error under each document that names the
/// source, and exits 4 as `check` does.
#[test]
fn explain_shows_a_source_that_cannot_be_hashed() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["config", "filter.custom.clean", "cat"]);
	repo.write(".gitattributes", "src/a.rs filter=custom\n");

	let check = repo.run(&["check", "--at", AT]);
	check.expect_code(4);
	assert_eq!(check.stdout.matches("unsupported-filter").count(), 1);

	let explain = repo.run(&["explain", "docs/guide.md", "--at", AT]);
	explain.expect_code(4);
	let (_, diagnostics) = explain
		.stdout
		.split_once("\nDiagnostics\n")
		.unwrap_or_else(|| panic!("no diagnostics in:\n{}", explain.stdout));
	assert!(
		diagnostics.contains("unsupported-filter  src/a.rs"),
		"{}",
		explain.stdout
	);
	// A source that cannot be hashed leaves the document with no fact: it is
	// not judged, and never called fresh.
	assert!(explain.stdout.contains("not judged"), "{}", explain.stdout);
	assert!(!explain.stdout.contains("fresh"), "{}", explain.stdout);

	let json = repo.run(&["check", "--at", AT, "--format", "json"]);
	json.expect_code(4);
	let report = json.json();
	let guide = report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject["document"] == "docs/guide.md")
		.unwrap_or_else(|| panic!("{report:#}"));
	assert_eq!(guide["facts"], serde_json::json!([]), "{report:#}");
}

/// `explain` names a pattern source as its declaration writes it, as `diff`
/// does: `/src/*.rs` in a nested project, whose `/` is that project's root,
/// whether it runs from the nested project or from the repository root.
#[test]
fn explain_names_a_pattern_as_declared() {
	let repo = TestRepo::init();
	repo.write("nested/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("nested/doc.md", "# Nested\n");
	repo.write("nested/src/b.rs", "fn b() {}\n");
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
		.run_in(
			"nested",
			&["track", "doc.md", "--pattern", "/src/*.rs", "--yes"],
		)
		.expect_code(0);
	repo
		.run(&["track", "docs/guide.md", "--pattern", "/src/*.rs", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");

	for (directory, document, member) in [
		("nested", "doc.md", "src/b.rs"),
		(".", "nested/doc.md", "nested/src/b.rs"),
		(".", "docs/guide.md", "src/a.rs"),
	] {
		let run = repo.run_in(directory, &["explain", document, "--at", AT]);
		let line = run
			.stdout
			.lines()
			.find(|line| line.trim_start().starts_with(member))
			.unwrap_or_else(|| panic!("no {member} in:\n{}", run.stdout));
		assert!(line.ends_with(", from /src/*.rs"), "{document}: {line}");
	}
}
