//! `seal` and `retire`.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo, document};

/// A repository whose `records/audit.md` is tracked in `records/source-sentry.lock`
/// and committed.
fn audit() -> TestRepo {
	let repo = TestRepo::init();
	repo.write("records/audit.md", "# Audit\nEverything was fine.\n");
	repo
		.run(&["track", "records/audit.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
}

fn lockfile(repo: &TestRepo) -> Value {
	serde_json::from_str(&repo.read("records/source-sentry.lock")).unwrap()
}

/// A sealed record is fresh while it is intact, and a changed
/// record is an integrity error.
#[test]
fn seal_makes_a_record_that_must_not_change() {
	let repo = audit();
	let run =
		repo.run(&["seal", "records/audit.md", "--by", "Carla Mendes", "--yes"]);
	run.expect_code(0);
	assert!(
		run.stdout.contains("Seal records/audit.md"),
		"{}",
		run.stdout
	);
	let seal = &lockfile(&repo)["documents"]["audit.md"]["seal"];
	assert_eq!(seal["by"], "Carla Mendes");
	assert_eq!(
		seal["oid"],
		format!("sha1:{}", repo.git(&["hash-object", "records/audit.md"]))
	);
	repo.commit_at("2026-10-02T10:00:00Z");

	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let record = document(&report, "records/audit.md");
	assert_eq!(record["facts"], json!(["fresh"]));
	assert_eq!(record["record"]["state"], "intact");
	repo.run(&["check", "--at", AT]).expect_code(0);

	repo.write(
		"records/audit.md",
		"# Audit\nEverything was fine, mostly.\n",
	);
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(3);
	assert!(run.stdout.contains("broken-seal"), "{}", run.stdout);

	let again = repo.run(&["seal", "records/audit.md", "--yes"]);
	assert_ne!(again.code, 0, "a record is never resealed");
}

/// `--by` defaults to git's `user.name`.
#[test]
fn seal_records_the_git_user_by_default() {
	let repo = audit();
	repo
		.run(&["seal", "records/audit.md", "--yes"])
		.expect_code(0);
	assert_eq!(
		lockfile(&repo)["documents"]["audit.md"]["seal"]["by"],
		"Test Author"
	);
}

/// Retiring moves the entry to `retired`, keeping its
/// seal, and leaves the document alone.
#[test]
fn retire_leaves_a_tombstone_with_the_seal() {
	let repo = audit();
	repo
		.run(&["seal", "records/audit.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	let sealed = lockfile(&repo)["documents"]["audit.md"]["seal"].clone();

	let run =
		repo.run(&["retire", "records/audit.md", "--by", "Bruno Costa", "--yes"]);
	run.expect_code(0);
	assert!(run.stdout.contains("becomes a tombstone"), "{}", run.stdout);
	let after = lockfile(&repo);
	assert_eq!(after["documents"], json!({}));
	assert_eq!(after["retired"]["audit.md"][0]["seal"], sealed);
	assert_eq!(
		after["retired"]["audit.md"][0]["retired"]["by"],
		"Bruno Costa"
	);
	assert!(
		repo.exists("records/audit.md"),
		"the document is not touched"
	);

	let again = repo.run(&["retire", "records/audit.md", "--yes"]);
	again.expect_code(2);
	assert!(again.stderr.contains("not tracked"), "{}", again.stderr);
}
