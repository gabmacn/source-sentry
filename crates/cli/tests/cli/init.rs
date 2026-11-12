//! `init` and `migrate`.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo, document};

/// A repository whose `docs/` is a root with two untracked candidates.
fn adoptable() -> TestRepo {
	let repo = TestRepo::init();
	repo.write("docs/source-sentry.yaml", "root: true\n");
	repo.write("docs/other.md", "# Other\n");
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
}

#[test]
fn init_without_paths_only_previews() {
	let repo = adoptable();
	let run = repo.run(&["init"]);
	run.expect_code(0);
	for expected in [
		"Roots  docs",
		"Tracked  none yet",
		"Candidates (2)",
		"docs/guide.md  in root docs",
	] {
		assert!(
			run.stdout.contains(expected),
			"missing {expected:?}:\n{}",
			run.stdout
		);
	}
	assert!(!repo.exists("docs/source-sentry.lock"));
}

/// Tracked documents are counted by placement, not listed.
#[test]
fn the_preview_counts_tracked_documents() {
	let repo = adoptable();
	repo.run(&["init", "docs/guide.md", "--yes"]).expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	let run = repo.run(&["init"]);
	run.expect_code(0);
	assert!(
		run.stdout.contains("Tracked  1 (1 lockfile:docs)"),
		"{}",
		run.stdout
	);
	assert!(!run.stdout.contains("docs/guide.md"), "{}", run.stdout);
	assert!(
		run.stdout.contains("docs/other.md  in root docs"),
		"{}",
		run.stdout
	);
}

/// Adoption registers documents as unattested, never as
/// reviewed.
#[test]
fn init_registers_candidates_as_unattested() {
	let repo = adoptable();
	repo
		.run(&["init", "docs/guide.md", "--owner", "platform", "--yes"])
		.expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lockfile["documents"],
		json!({"guide.md": {"owners": ["platform"]}})
	);

	// The new lockfile is marked intent-to-add, so a second run
	// sees it without `git add`.
	let listed = repo.git(&["ls-files", "--", "docs/source-sentry.lock"]);
	assert_eq!(listed.trim(), "docs/source-sentry.lock");
	repo.run(&["init", "--all", "--yes"]).expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert!(
		lockfile["documents"]["other.md"].is_object(),
		"{lockfile:#}"
	);

	repo.commit_at("2026-10-02T10:00:00Z");
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	assert_eq!(
		document(&report, "docs/other.md")["facts"],
		json!(["overdue", "unattested"])
	);

	let nothing = repo.run(&["init", "--all", "--yes"]);
	nothing.expect_code(0);
	assert!(
		nothing.stdout.contains("Nothing to change."),
		"{}",
		nothing.stdout
	);
}

#[test]
fn migrate_reports_current_lockfiles() {
	let repo = adoptable();
	repo.run(&["init", "--all", "--yes"]).expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	let run = repo.run(&["migrate"]);
	run.expect_code(0);
	assert!(
		run.stdout.contains("docs/source-sentry.lock  schema 1"),
		"{}",
		run.stdout
	);
	assert!(run.stdout.contains("Nothing to change."), "{}", run.stdout);
}
