//! Output that cannot be written: a command whose standard output has no
//! reader exits 4 and says so on standard error, whatever its result.

use crate::support::{AT, TestRepo};

/// What standard error says when standard output cannot be written.
const WRITE_ERROR: &str =
	"error: could not write the output to standard output: ";

/// A repository whose guide is stale: its source changed after review, so
/// `check --fail-on warn` exits non-zero.
fn stale_guide() -> TestRepo {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() { changed() }\n");
	repo.commit_at("2026-10-03T10:00:00Z");
	repo
}

/// Asserts that a run ended because its output could not be written.
#[track_caller]
fn assert_write_failure(run: &crate::support::Run) {
	run.expect_code(4);
	let lines: Vec<&str> = run.stderr.lines().collect();
	assert_eq!(lines.len(), 1, "{}", run.stderr);
	assert!(lines[0].starts_with(WRITE_ERROR), "{}", run.stderr);
	assert!(
		lines[0].len() > WRITE_ERROR.len(),
		"the system's reason follows: {}",
		run.stderr
	);
}

#[test]
fn a_json_report_that_cannot_be_written_exits_4() {
	let repo = stale_guide();
	let args = ["check", "--at", AT, "--fail-on", "warn", "--format", "json"];
	let ordinary = repo.run(&args);
	assert_ne!(ordinary.code, 0, "{}", ordinary.stdout);
	assert_ne!(ordinary.code, 4, "{}", ordinary.stderr);
	assert_write_failure(&repo.run_without_reader(&args));
}

#[test]
fn a_passing_json_report_that_cannot_be_written_exits_4() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let args = ["check", "--at", AT, "--format", "json"];
	repo.run(&args).expect_code(0);
	assert_write_failure(&repo.run_without_reader(&args));
}

#[test]
fn human_output_that_cannot_be_written_exits_4() {
	let repo = stale_guide();
	repo
		.run(&["explain", "docs/guide.md", "--at", AT])
		.expect_code(0);
	assert_write_failure(&repo.run_without_reader(&[
		"explain",
		"docs/guide.md",
		"--at",
		AT,
	]));
}

#[test]
fn output_that_is_read_keeps_the_commands_exit_code() {
	let repo = stale_guide();
	let run =
		repo.run(&["check", "--at", AT, "--fail-on", "warn", "--format", "json"]);
	assert!(matches!(run.code, 1 | 2), "{}", run.stdout);
	assert_eq!(run.json()["exit-code"], run.code, "{}", run.stdout);
	assert_eq!(run.stderr, "");
}
