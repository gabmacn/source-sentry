//! Clocks and completions that must survive a declaration moving to another
//! lockfile, or a series entry being retired.

use crate::support::TestRepo;

/// A repository whose default context fails overdue documents and series.
fn failing_overdue() -> TestRepo {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    default:\n      fallback: notice\n      \
		 rules:\n        - facts: [overdue]\n          outcome: fail\n",
	);
	repo
}

#[test]
fn moving_a_remote_document_keeps_its_clock() {
	let repo = failing_overdue();
	repo.write("src.dat", "x\n");
	repo.commit_at("2026-11-01T12:00:00Z");
	repo
		.run(&[
			"track",
			"abstract:video",
			"--remote",
			"legal",
			"--source",
			"/src.dat",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-11-02T12:00:00Z");
	repo
		.run(&[
			"track",
			"abstract:video",
			"--from",
			"remote:legal",
			"--to",
			"remote:marketing",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-11-20T12:00:00Z");

	let run = repo.run(&["check", "--at", "2026-12-05T12:00:00Z", "--no-cache"]);
	run.expect_code(2);
	assert!(
		run.stdout.contains("unattested since 2026-11-02"),
		"{}",
		run.stdout
	);
}

#[test]
fn moving_a_local_document_to_a_nested_lockfile_keeps_its_clock() {
	let repo = failing_overdue();
	repo.commit_at("2026-11-01T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--lockfile",
			".",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-11-02T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--from",
			"lockfile:.",
			"--to",
			"lockfile:docs",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-11-20T12:00:00Z");

	let run = repo.run(&["check", "--at", "2026-12-05T12:00:00Z", "--no-cache"]);
	run.expect_code(2);
	assert!(
		run.stdout.contains("unattested since 2026-11-02"),
		"{}",
		run.stdout
	);
}

#[test]
fn a_moved_tombstone_keeps_its_completion_date() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		r"version: 1
time-zone: UTC
series:
  audits:
    pattern: 'records/audit-(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: records-only
    schedule:
      cadence:
        dtstart: 2026-11-01
        rrule: 'FREQ=MONTHLY'
",
	);
	repo.write("records/audit-1.md", "first\n");
	repo.commit_at("2026-11-10T12:00:00Z");
	repo
		.run(&["track", "records/audit-1.md", "--lockfile", ".", "--yes"])
		.expect_code(0);
	repo
		.run(&["seal", "records/audit-1.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-11-12T12:00:00Z");
	repo
		.run(&["retire", "records/audit-1.md", "--yes"])
		.expect_code(0);
	repo.git(&["rm", "--quiet", "records/audit-1.md"]);
	repo.commit_at("2026-11-14T12:00:00Z");
	repo.write("records/audit-2.md", "second\n");
	repo.git(&["add", "records/audit-2.md"]);
	repo
		.run(&[
			"track",
			"records/audit-2.md",
			"--lockfile",
			"records",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-12-10T12:00:00Z");

	let run = repo.run(&["explain", "audits", "--at", "2027-01-02T12:00:00Z"]);
	assert!(
		run.stdout.contains("counted from 2026-11-12"),
		"{}",
		run.stdout
	);
	assert!(
		run.stdout.contains(
			"cadence date 2027-01-01, last day 2027-01-01: closed without an entry: overdue"
		),
		"{}",
		run.stdout
	);
}

#[test]
fn retiring_a_completed_living_entry_keeps_its_credit() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		r"version: 1
time-zone: UTC
series:
  proposals:
    pattern: 'proposals/v(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: living
    schedule:
      cadence:
        dtstart: 2026-11-01
        rrule: 'FREQ=MONTHLY'
",
	);
	repo.write("proposals/v1.md", "first\n");
	repo.commit_at("2026-11-10T12:00:00Z");
	repo
		.run(&["track", "proposals/v1.md", "--yes"])
		.expect_code(0);
	repo
		.run(&["attest", "proposals/v1.md", "--reread", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-11-12T12:00:00Z");
	repo.write("proposals/v2.md", "second\n");
	repo.git(&["add", "proposals/v2.md"]);
	repo
		.run(&["track", "proposals/v2.md", "--yes"])
		.expect_code(0);
	repo
		.run(&["seal", "proposals/v1.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-11-14T12:00:00Z");
	repo
		.run(&["retire", "proposals/v1.md", "--yes"])
		.expect_code(0);
	repo.git(&["rm", "--quiet", "proposals/v1.md"]);
	repo.commit_at("2026-11-15T12:00:00Z");

	let run = repo.run(&["explain", "proposals", "--at", "2026-12-02T12:00:00Z"]);
	assert!(
		run.stdout.contains(
			"cadence date 2026-12-01, last day 2026-12-01: credited to proposals/v1.md"
		),
		"{}",
		run.stdout
	);
}

#[test]
fn a_credited_series_shows_its_next_deadline() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		r"version: 1
series:
  reports:
    pattern: 'reports/report-(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: living
    schedule:
      cadence:
        dtstart: 2026-02-01
        rrule: 'FREQ=MONTHLY'
",
	);
	repo.write("reports/report-1.md", "report\n");
	repo.commit_at("2026-01-01T12:00:00Z");
	repo
		.run(&["track", "reports/report-1.md", "--yes"])
		.expect_code(0);
	repo
		.run(&["attest", "reports/report-1.md", "--reread", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");

	let run = repo.run(&["explain", "reports", "--at", "2026-01-02T13:00:00Z"]);
	assert!(
		run.stdout.contains("credited to reports/report-1.md"),
		"{}",
		run.stdout
	);
	assert!(run.stdout.contains("entry by 2026-03-01"), "{}", run.stdout);
	assert!(!run.stdout.contains("no clock runs"), "{}", run.stdout);
}

#[test]
fn a_move_out_of_frontmatter_keeps_its_clock_when_the_adapter_changes() {
	let repo = failing_overdue();
	repo.write("src.dat", "source\n");
	repo.commit_at("2026-01-01T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--on-file",
			"--source",
			"/src.dat",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--from",
			"on-file",
			"--to",
			"lockfile:.",
			"--yes",
		])
		.expect_code(0);
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    default:\n      fallback: notice\n      \
		 rules:\n        - facts: [overdue]\n          outcome: fail\ndocuments:\n  \
		 adapters:\n    - match: '*.md'\n      adapter: opaque\n",
	);
	repo.commit_at("2026-01-20T12:00:00Z");

	let run = repo.run(&["check", "--at", "2026-02-10T00:00:00Z", "--no-cache"]);
	run.expect_code(2);
	assert!(
		run.stdout.contains("unattested since 2026-01-02"),
		"{}",
		run.stdout
	);
}
