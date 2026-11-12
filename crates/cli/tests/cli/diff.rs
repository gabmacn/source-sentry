//! Copyable diff commands keep shell syntax in file names literal.

use crate::support::TestRepo;

/// The printed command is replayable through PowerShell or the POSIX shell.
#[test]
fn printed_diff_commands_quote_special_file_names() {
	for prefix in ["@", ",", "#"] {
		let repo = TestRepo::init();
		let document = format!("{prefix}doc.md");
		let source = format!("{prefix}source.rs");
		repo.write(&document, "# Guide\n");
		repo.write(&source, "hello\n");
		repo.commit_at("2026-10-01T10:00:00Z");
		repo
			.run(&["track", &document, "--source", &source, "--yes"])
			.expect_code(0);
		repo.run(&["attest", &document, "--dry-run"]).expect_code(0);
		let before = repo.read("source-sentry.lock");
		let run = repo.run(&["diff", &document]);
		run.expect_code(0);
		let command = run
			.stdout
			.lines()
			.last()
			.expect("the last line is the attestation command");
		repo
			.run_shell_in(".", &format!("{command} --dry-run"))
			.expect_code(0);
		assert_eq!(repo.read("source-sentry.lock"), before);
	}
}
