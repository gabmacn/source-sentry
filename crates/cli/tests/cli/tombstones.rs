//! Repeated retirement, series credit and moves of whole tombstone lists.

use serde_json::{Value, json};

use crate::support::TestRepo;

/// Reads a test lockfile as JSON.
fn lockfile(repo: &TestRepo, file: &str) -> Value {
	serde_json::from_str(&repo.read(file)).unwrap()
}

/// Checks both closed windows and their introducing commits after repeated
/// retirement, including after the tombstones move to another lockfile.
fn assert_credited(repo: &TestRepo) {
	let report = repo
		.run(&[
			"check",
			"--at",
			"2026-12-02T12:00:00Z",
			"--format",
			"json",
			"--fail-on",
			"fail",
		])
		.expect_code(0)
		.json();
	assert_eq!(report["diagnostics"], json!([]));
	let series = report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject["name"] == "audits")
		.unwrap();
	assert_eq!(series["missed"], json!([]));
	assert_eq!(series["facts"], json!([]));
	let explanation =
		repo.run(&["explain", "audits", "--at", "2026-12-02T12:00:00Z"]);
	explanation.expect_code(0);
	for date in ["2026-10-10", "2026-11-10"] {
		assert!(
			explanation.stdout.contains(&format!("counted from {date}")),
			"{}",
			explanation.stdout
		);
	}
	for due in ["2026-11-01", "2026-12-01"] {
		assert!(
			explanation.stdout.contains(&format!(
				"due {due}, last day {due}: credited to records/audit-1.md"
			)),
			"{}",
			explanation.stdout
		);
	}
}

/// Tracks, seals and retires one incarnation, committing its marker and
/// tombstone at separate instants. Living entries are completed first.
fn retire_generation(
	repo: &TestRepo,
	lifecycle: &str,
	content: &str,
	reviewer: &str,
	completed_at: &str,
	retired_at: &str,
) -> Value {
	repo.write("records/audit-1.md", content);
	repo
		.run(&["track", "records/audit-1.md", "--yes"])
		.expect_code(0);
	if lifecycle == "living" {
		repo
			.run(&[
				"attest",
				"records/audit-1.md",
				"--reread",
				"--reviewer",
				reviewer,
				"--yes",
			])
			.expect_code(0);
	}
	repo
		.run(&["seal", "records/audit-1.md", "--by", reviewer, "--yes"])
		.expect_code(0);
	let declaration =
		lockfile(repo, "records/source-sentry.lock")["documents"]["audit-1.md"]
			.clone();
	repo.commit_at(completed_at);
	repo
		.run(&["retire", "records/audit-1.md", "--by", reviewer, "--yes"])
		.expect_code(0);
	repo.git(&["rm", "--quiet", "records/audit-1.md"]);
	repo.commit_at(retired_at);
	declaration
}

/// A path can be retired, reused and retired again in either series
/// lifecycle. Both seals and completion windows survive a later move.
#[test]
fn repeated_retirement_keeps_all_seals_and_series_windows_through_a_move() {
	for lifecycle in ["records-only", "living"] {
		let repo = TestRepo::init();
		repo.write(
			".source-sentry/source-sentry.yaml",
			&format!(
				r"version: 1
time-zone: UTC
series:
  audits:
    pattern: 'records/audit-(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: {lifecycle}
    schedule:
      cadence:
        dtstart: 2026-11-01
        rrule: 'FREQ=MONTHLY'
"
			),
		);
		if lifecycle == "living" {
			repo.write("records/audit-2.md", "head\n");
			repo
				.run(&["track", "records/audit-2.md", "--yes"])
				.expect_code(0);
		}
		let file = "records/source-sentry.lock";
		let mut seals = Vec::new();
		let mut completions = Vec::new();
		for (content, reviewer, completed, retired) in [
			(
				"first\n",
				"First reviewer",
				"2026-10-10T12:00:00Z",
				"2026-10-11T12:00:00Z",
			),
			(
				"second\n",
				"Second reviewer",
				"2026-11-10T12:00:00Z",
				"2026-11-11T12:00:00Z",
			),
		] {
			let declaration = retire_generation(
				&repo, lifecycle, content, reviewer, completed, retired,
			);
			seals.push(declaration["seal"].clone());
			completions.push(declaration["completed"].clone());
		}
		let before = lockfile(&repo, file);
		let tombstones = before["retired"]["audit-1.md"].as_array().unwrap();
		assert_eq!(tombstones.len(), 2);
		for (index, tombstone) in tombstones.iter().enumerate() {
			assert_eq!(tombstone["seal"], seals[index]);
			assert_eq!(tombstone["completed"], completions[index]);
		}
		assert_credited(&repo);

		// A third declaration moves with both tombstones of its key.
		repo.write("records/audit-1.md", "third\n");
		repo
			.run(&["track", "records/audit-1.md", "--yes"])
			.expect_code(0);
		let mut command = vec!["track", "records/audit-1.md"];
		if lifecycle == "living" {
			command.push("records/audit-2.md");
		}
		command.extend([
			"--from",
			"lockfile:records",
			"--to",
			"lockfile:.",
			"--yes",
		]);
		repo.run(&command).expect_code(0);
		let moved = lockfile(&repo, "source-sentry.lock");
		assert_eq!(
			moved["retired"]["records/audit-1.md"],
			before["retired"]["audit-1.md"]
		);
		assert!(!repo.exists(file));
		repo.commit_at("2026-11-12T12:00:00Z");
		assert_credited(&repo);
	}
}

/// Remote identities can also be reused repeatedly, and moving their next
/// declaration carries the whole list to the new owner.
#[test]
fn repeated_remote_retirements_move_together() {
	let repo = TestRepo::init();
	for reviewer in ["First reviewer", "Second reviewer"] {
		repo
			.run(&["track", "abstract:video", "--remote", "legal", "--yes"])
			.expect_code(0);
		repo
			.run(&["retire", "abstract:video", "--by", reviewer, "--yes"])
			.expect_code(0);
	}
	let before = lockfile(&repo, ".source-sentry/remotes/legal.lock");
	assert_eq!(
		before["retired"]["abstract:video"]
			.as_array()
			.unwrap()
			.len(),
		2
	);
	repo
		.run(&["track", "abstract:video", "--remote", "legal", "--yes"])
		.expect_code(0);
	repo
		.run(&[
			"track",
			"abstract:video",
			"--from",
			"remote:legal",
			"--to",
			"remote:training",
			"--yes",
		])
		.expect_code(0);
	let after = lockfile(&repo, ".source-sentry/remotes/training.lock");
	assert_eq!(after["retired"], before["retired"]);
	assert!(!repo.exists(".source-sentry/remotes/legal.lock"));
	repo
		.run(&["check", "--format", "json", "--fail-on", "fail"])
		.expect_code(0);
}
