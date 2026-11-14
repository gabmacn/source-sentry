//! Pattern reviews remain recorded after their final member is removed.

use serde_json::{Value, json};

use crate::support::{TestRepo, document};

/// A removal acknowledgement retains source review evidence without a reread.
#[test]
fn acknowledging_the_last_pattern_removal_keeps_the_document_reviewed() {
	let repo = emptied_pattern();
	let pending =
		repo.run(&["check", "--format", "json", "--at", "2026-02-10T12:00:00Z"]);
	pending.expect_code(1);
	let report = pending.json();
	let guide = document(&report, "guide.md");
	assert_eq!(guide["facts"], json!(["stale", "due", "overdue"]));
	assert_eq!(guide["deadline"]["due"], "2026-02-02");
	repo
		.run(&[
			"attest",
			"guide.md",
			"--expect",
			"/src/a.rs@absent",
			"--reviewer",
			"QA",
			"--yes",
		])
		.expect_code(0);
	assert_reviewed_empty_pattern(&repo);
	repo.commit_at("2026-01-04T12:00:00Z");
	assert_reviewed_empty_pattern(&repo);
}

/// A previously reviewed empty pattern permits an explicit whole-document reread.
#[test]
fn rereading_an_emptied_pattern_succeeds_after_acknowledgement() {
	let repo = emptied_pattern();
	repo
		.run(&[
			"attest",
			"guide.md",
			"--expect",
			"/src/a.rs@absent",
			"--reviewer",
			"QA",
			"--yes",
		])
		.expect_code(0);
	repo
		.run(&[
			"attest",
			"guide.md",
			"--reread",
			"--reviewer",
			"QA",
			"--yes",
		])
		.expect_code(0);
	let lockfile: Value = serde_json::from_str(&repo.read("source-sentry.lock"))
		.expect("lockfile JSON");
	let declaration = &lockfile["documents"]["guide.md"];
	assert_eq!(declaration["reviewed"]["by"], "QA");
	assert_eq!(
		declaration["sources"][0]["reviewed"],
		declaration["reviewed"]
	);
}

/// A reviewed pattern with its only member deleted in a dated commit.
fn emptied_pattern() -> TestRepo {
	let repo = TestRepo::init();
	repo.git(&["rm", "-f", "docs/guide.md"]);
	repo.write("guide.md", "# Guide\n");
	repo.commit_at("2026-01-01T12:00:00Z");
	repo
		.run(&[
			"track",
			"guide.md",
			"--pattern",
			"/src/*.rs",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo
		.run(&["attest", "guide.md", "--reviewer", "QA", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");
	repo.git(&["rm", "src/a.rs"]);
	repo.commit_at("2026-01-03T12:00:00Z");
	repo
}

/// Only the empty-pattern warning remains, without document reread evidence.
fn assert_reviewed_empty_pattern(repo: &TestRepo) {
	let run =
		repo.run(&["check", "--format", "json", "--at", "2026-02-10T12:00:00Z"]);
	run.expect_code(0);
	let report = run.json();
	let guide = document(&report, "guide.md");
	assert_eq!(guide["facts"], json!(["fresh"]));
	assert!(guide["deadline"].is_null());
	assert!(guide["unattested"].is_null());
	let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
	assert_eq!(diagnostics.len(), 1);
	assert_eq!(diagnostics[0]["code"], "empty-pattern");
	let lockfile: Value = serde_json::from_str(&repo.read("source-sentry.lock"))
		.expect("lockfile JSON");
	let declaration = &lockfile["documents"]["guide.md"];
	assert!(declaration.get("reviewed").is_none());
	assert_eq!(declaration["sources"][0]["reviewed"]["by"], "QA");
	assert!(declaration["sources"][0].get("members").is_none());
	let diff = repo.run(&["diff", "guide.md"]);
	diff.expect_code(0);
	assert!(diff.stdout.contains("its previous review is retained"));
	assert!(!diff.stdout.contains("was never attested"));
	let diff = repo.run(&["diff", "guide.md", "--format", "json"]);
	diff.expect_code(0);
	let diff = diff.json();
	assert_eq!(
		diff["sources"][0]["reviewed"],
		declaration["sources"][0]["reviewed"]
	);
	assert_eq!(diff["sources"][0]["entries"], json!([]));
	assert_eq!(diff["expectations"], json!([]));
}
