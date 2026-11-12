//! `rename` against real git moves, including current-directory paths.

use std::fs;

use serde_json::Value;

use crate::support::{AT, TestRepo, document};

/// Reads a lockfile as JSON for comparisons that ignore formatting.
fn lock(repo: &TestRepo, path: &str) -> Value {
	serde_json::from_str(&repo.read(path)).unwrap()
}

/// A confirmed move preserves every review, rebases relative sources and
/// uses the destination lockfile even when invoked from a subdirectory.
#[test]
fn rename_from_a_subdirectory_keeps_reviews_and_changes_authority() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--source", "src/a.rs", "--yes"])
		.expect_code(0);
	repo
		.run(&["attest", "docs/guide.md", "--reread", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let before =
		lock(&repo, "docs/source-sentry.lock")["documents"]["guide.md"].clone();
	repo.write("archive/other.md", "# Other\n");
	repo
		.run(&["track", "archive/other.md", "--yes"])
		.expect_code(0);
	fs::create_dir_all(repo.path().join("archive/deep")).unwrap();
	repo.git(&["mv", "docs/guide.md", "archive/deep/renamed.md"]);
	let result = repo.run_in(
		"docs",
		&["rename", "guide.md", "../archive/deep/renamed.md", "--yes"],
	);
	result.expect_code(0);
	assert!(
		result
			.stdout
			.contains("Rename guide.md to ../archive/deep/renamed.md"),
		"{}",
		result.stdout
	);
	assert!(result.stdout.contains("../archive/source-sentry.lock"));
	assert!(!repo.exists("docs/source-sentry.lock"));
	let after =
		lock(&repo, "archive/source-sentry.lock")["documents"]["deep/renamed.md"]
			.clone();
	assert_eq!(after["reviewed"], before["reviewed"]);
	assert_eq!(after["sources"][0]["oid"], before["sources"][0]["oid"]);
	assert_eq!(
		after["sources"][0]["reviewed"],
		before["sources"][0]["reviewed"]
	);
	assert_eq!(after["sources"][0]["path"], "../../src/a.rs");
	let check = repo.run(&["check", "--at", AT, "--format", "json"]);
	assert_eq!(
		document(&check.json(), "archive/deep/renamed.md")["facts"],
		serde_json::json!(["fresh"])
	);
}

/// Dry runs and lack of confirmation leave every record untouched.
#[test]
fn rename_requires_confirmation_and_dry_run_writes_nothing() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["mv", "docs/guide.md", "docs/new.md"]);
	let before = repo.read("docs/source-sentry.lock");
	for options in [&["--dry-run"][..], &["--dry-run", "--yes"], &[]] {
		let mut args = vec!["rename", "docs/guide.md", "docs/new.md"];
		args.extend_from_slice(options);
		let result = repo.run(&args);
		result.expect_code(i32::from(options.is_empty()));
		assert!(
			result
				.stdout
				.contains("Rename docs/guide.md to docs/new.md")
		);
		assert_eq!(repo.read("docs/source-sentry.lock"), before);
	}
}

/// Existence, git membership, an old declaration, a free destination and
/// local identities are all checked before writing.
#[test]
fn rename_precondition_refusals_are_actionable() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("docs/new.md", "# New\n");
	let before = repo.read("docs/source-sentry.lock");
	let existing = repo.run(&["rename", "docs/guide.md", "docs/new.md", "--yes"]);
	existing.expect_code(2);
	assert!(existing.stderr.contains("git mv"));
	for args in [
		["abstract:guide", "docs/new.md"],
		["docs/missing.md", "docs/new.md"],
		["docs/missing.md", "docs/absent.md"],
	] {
		repo
			.run(&["rename", args[0], args[1], "--yes"])
			.expect_code(2);
	}
	fs::remove_file(repo.path().join("docs/guide.md")).unwrap();
	fs::write(repo.path().join("docs/untracked.md"), "# Guide\n").unwrap();
	let untracked =
		repo.run(&["rename", "docs/guide.md", "docs/untracked.md", "--yes"]);
	untracked.expect_code(2);
	assert!(untracked.stderr.contains("git add -N"));
	let local =
		repo.run_in("docs", &["rename", "guide.md", "untracked.md", "--yes"]);
	local.expect_code(2);
	assert!(
		local
			.stderr
			.contains("cannot rename `guide.md` to `untracked.md`"),
		"{}",
		local.stderr
	);
	assert_eq!(repo.read("docs/source-sentry.lock"), before);
	repo.run(&["track", "docs/new.md", "--yes"]).expect_code(0);
	repo
		.run(&["rename", "docs/guide.md", "docs/new.md", "--yes"])
		.expect_code(2);
}

/// Intent-to-add is sufficient after a filesystem move.
#[test]
fn rename_accepts_an_intent_to_add_destination() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	fs::rename(
		repo.path().join("docs/guide.md"),
		repo.path().join("docs/new.md"),
	)
	.unwrap();
	repo.git(&["add", "-N", "docs/new.md"]);
	repo
		.run(&["rename", "docs/guide.md", "docs/new.md", "--yes"])
		.expect_code(0);
	assert!(
		lock(&repo, "docs/source-sentry.lock")["documents"]["new.md"].is_object()
	);
}

/// Relative on-file sources can move toward the repository root even when
/// their old spellings temporarily climb above the destination directory.
#[test]
fn rename_rebases_carried_frontmatter_before_validating_its_new_directory() {
	let repo = TestRepo::init();
	repo.write("docs/deep/guide.md", "---\ntitle: Keep me\n---\n# Body\n");
	repo
		.run(&[
			"track",
			"docs/deep/guide.md",
			"--on-file",
			"--source",
			"src/a.rs",
			"--yes",
		])
		.expect_code(0);
	repo
		.run(&["attest", "docs/deep/guide.md", "--reread", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let before = repo.read("docs/deep/guide.md");
	repo.git(&["mv", "docs/deep/guide.md", "guide.md"]);
	repo
		.run(&["rename", "docs/deep/guide.md", "guide.md", "--yes"])
		.expect_code(0);
	let after = repo.read("guide.md");
	assert!(after.contains("title: Keep me\n"));
	assert!(after.ends_with("---\n# Body\n"));
	assert!(after.contains("path: 'src/a.rs'"), "{after}");
	assert_eq!(after, before.replace("../../src/a.rs", "src/a.rs"));
	assert!(!repo.exists("source-sentry.lock"));
}

/// Path references and matching reviewed members follow the renamed file;
/// a pattern that ceases to match remains available for review.
#[test]
fn rename_updates_dependents_and_previews_unmatched_patterns() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	for (name, option, source) in [
		("path.txt", "--source", "docs/guide.md"),
		("pattern.txt", "--pattern", "/**/*.md"),
		("old.txt", "--pattern", "/docs/*.md"),
	] {
		repo.write(name, "Reader\n");
		repo
			.run(&["track", name, option, source, "--yes"])
			.expect_code(0);
		repo
			.run(&["attest", name, "--reread", "--yes"])
			.expect_code(0);
	}
	let before = lock(&repo, "source-sentry.lock");
	fs::create_dir_all(repo.path().join("archive")).unwrap();
	repo.git(&["mv", "docs/guide.md", "archive/guide.md"]);
	let result =
		repo.run(&["rename", "docs/guide.md", "archive/guide.md", "--yes"]);
	result.expect_code(0);
	assert!(
		result.stdout.contains("leave it unchanged for `check`"),
		"{}",
		result.stdout
	);
	let after = lock(&repo, "source-sentry.lock");
	assert_eq!(
		after["documents"]["path.txt"]["sources"][0]["path"],
		"archive/guide.md"
	);
	assert_eq!(
		after["documents"]["path.txt"]["sources"][0]["oid"],
		before["documents"]["path.txt"]["sources"][0]["oid"]
	);
	assert_eq!(
		after["documents"]["pattern.txt"]["sources"][0]["members"][0]["path"],
		"/archive/guide.md"
	);
	assert_eq!(
		after["documents"]["old.txt"],
		before["documents"]["old.txt"]
	);
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	assert_eq!(
		document(&report, "pattern.txt")["sources"][0]["state"],
		"fresh"
	);
	assert_eq!(
		document(&report, "old.txt")["sources"][0]["state"],
		"removed"
	);
}

/// The general missing-document hint uses the caller's directory and
/// preserves `--repo` when it was supplied.
#[test]
fn missing_document_hints_use_current_directory_and_command_prefix() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["mv", "docs/guide.md", "docs/new.md"]);
	let check = repo.run_in("docs", &["--repo", "..", "check", "--at", AT]);
	check.expect_code(3);
	assert!(
		check
			.stdout
			.contains("source-sentry --repo .. rename guide.md <new>"),
		"{}",
		check.stdout
	);
}

/// A sealed record keeps its exact seal through a move.
#[test]
fn rename_preserves_a_sealed_record() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.run(&["seal", "docs/guide.md", "--yes"]).expect_code(0);
	let before = lock(&repo, "docs/source-sentry.lock");
	repo.git(&["mv", "docs/guide.md", "docs/record.md"]);
	repo
		.run(&["rename", "docs/guide.md", "docs/record.md", "--yes"])
		.expect_code(0);
	let after = lock(&repo, "docs/source-sentry.lock");
	assert_eq!(
		after["documents"]["record.md"],
		before["documents"]["guide.md"]
	);
}

/// Version changes appear in the confirmation plan, and a rename cannot
/// promote a sealed entry into the living head of a series.
#[test]
fn rename_previews_series_positions_and_refuses_a_sealed_head() {
	let repo = TestRepo::init();
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\nseries:\n  proposals:\n    pattern: 'proposals/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: living\n");
	repo.write("proposals/v1.md", "# First\n");
	repo.write("proposals/v2.md", "# Second\n");
	repo
		.run(&["track", "proposals/v1.md", "--yes"])
		.expect_code(0);
	repo
		.run(&["track", "proposals/v2.md", "--yes"])
		.expect_code(0);
	repo
		.run(&["seal", "proposals/v1.md", "--yes"])
		.expect_code(0);
	repo.git(&["mv", "proposals/v2.md", "proposals/v3.md"]);
	let plan =
		repo.run(&["rename", "proposals/v2.md", "proposals/v3.md", "--dry-run"]);
	plan.expect_code(0);
	assert!(
		plan.stdout.contains(
			"change series `proposals` (the repository) from version 2 to 3"
		),
		"{}",
		plan.stdout
	);
	repo.git(&["mv", "proposals/v3.md", "proposals/other.md"]);
	let before = repo.read("proposals/source-sentry.lock");
	let refused =
		repo.run(&["rename", "proposals/v2.md", "proposals/other.md", "--yes"]);
	refused.expect_code(3);
	assert!(refused.stderr.contains("sealed head"), "{}", refused.stderr);
	assert_eq!(repo.read("proposals/source-sentry.lock"), before);
}
