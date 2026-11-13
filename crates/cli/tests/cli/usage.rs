//! Usage: invalid command lines exit 64, help and the version exit 0.

use crate::support::TestRepo;

#[test]
fn invalid_usage_exits_64() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	for args in [
		&[][..],
		&["frobnicate"],
		&["check", "--frobnicate"],
		&["check", "--fail-on", "maybe"],
		&["check", "--at", "yesterday"],
		&["check", "--format", "xml"],
		&["--color", "sometimes", "check"],
		&["attest"],
		&["rename"],
		&["rename", "docs/guide.md"],
		&["attest", "docs/guide.md", "--expect", "src/a.rs"],
		&["attest", "docs/guide.md", "--expect", "src/a.rs@sha1:xyz"],
		// Reviews cannot be backdated.
		&["attest", "docs/guide.md", "--at", "2026-10-03T09:30:00Z"],
		&["track", "docs/guide.md", "docs/other.md"],
		&["track", "docs/guide.md", "--from", "on-file"],
		&["track", "docs/guide.md", "--to", "lockfile:docs"],
		&[
			"track",
			"docs/guide.md",
			"--from",
			"elsewhere",
			"--to",
			"on-file",
		],
		&["track", "docs/guide.md", "--on-file", "--remote", "legal"],
		&["track", "docs/guide.md", "--cadence", "2026-12-01"],
		&[
			"track",
			"docs/guide.md",
			"--cadence",
			"soon",
			"FREQ=MONTHLY",
		],
		&["track", "docs/guide.md", "--class", "Not Kebab"],
		&["track", "abstract:intro-video"],
		&["track", "abstract:"],
		&["init", "docs/guide.md", "--all"],
		&["seal", "../outside.md"],
	] {
		let run = repo.run(args);
		assert_eq!(
			run.code, 64,
			"{args:?} should be a usage error\n--- stdout\n{}\n--- stderr\n{}",
			run.stdout, run.stderr
		);
		assert!(!run.stderr.is_empty(), "{args:?} explains the error");
	}
}

#[test]
fn help_and_version_exit_0() {
	let repo = TestRepo::init();
	for args in [
		&["--help"][..],
		&["-h"],
		&["help"],
		&["check", "--help"],
		&["help", "track"],
		&["rename", "--help"],
		&["--version"],
		&["-V"],
	] {
		let run = repo.run(args);
		assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
		assert!(!run.stdout.is_empty(), "{args:?} prints to stdout");
	}
	let help = repo.run(&["check", "--help"]);
	assert!(help.stdout.contains("--fail-on"), "{}", help.stdout);
	assert!(help.stdout.contains("64"), "the exit codes are listed");
	// The mark is for terminals; redirected help is plain text.
	let top = repo.run(&["--help"]);
	assert!(!top.stdout.contains('█'), "{}", top.stdout);
}

#[test]
fn outside_a_repository_the_check_cannot_run() {
	let directory = tempfile::tempdir().unwrap();
	let output = std::process::Command::new(env!("CARGO_BIN_EXE_source-sentry"))
		.args(["check", "--at", crate::support::AT])
		.current_dir(directory.path())
		.env("GIT_CEILING_DIRECTORIES", directory.path())
		.output()
		.unwrap();
	assert_eq!(output.status.code(), Some(4));
}
