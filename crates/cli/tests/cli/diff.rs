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

/// Runs the `attest` array of a JSON diff, with `--yes`, from the
/// document's project root, which the report names.
fn replay(repo: &TestRepo, report: &serde_json::Value) -> crate::support::Run {
	let mut command: Vec<_> = report["attest"]
		.as_array()
		.unwrap()
		.iter()
		.skip(1)
		.map(|word| word.as_str().unwrap())
		.collect();
	command.push("--yes");
	repo.run_in(report["project"].as_str().unwrap(), &command)
}

/// Whether the declared source `declared`, a pattern or not, has a file
/// changed since its attestation, in a JSON diff of every source.
fn changed(report: &serde_json::Value, declared: &str, pattern: bool) -> bool {
	report["sources"]
		.as_array()
		.unwrap()
		.iter()
		.find(|source| {
			source["declared"] == declared && source["pattern"] == pattern
		})
		.unwrap_or_else(|| panic!("no source `{declared}` in {report:#}"))["entries"]
		.as_array()
		.unwrap()
		.iter()
		.any(|entry| entry["changed"] == true)
}

/// The JSON `attest` array of an ordinary document records, from the
/// project root, the review of the source that was shown.
#[test]
fn the_json_attest_array_records_the_shown_source() {
	let repo = TestRepo::init();
	repo.write("src/b.rs", "fn b() {}\n");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--source",
			"/src/b.rs",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo
		.run_in(
			"docs",
			&[
				"diff",
				"guide.md",
				"--source",
				"../src/a.rs",
				"--format",
				"json",
			],
		)
		.expect_code(0)
		.json();
	replay(&repo, &report).expect_code(0);
	let after = repo
		.run(&["diff", "docs/guide.md", "--format", "json"])
		.expect_code(0)
		.json();
	assert!(!changed(&after, "/src/a.rs", false), "{after:#}");
	assert!(changed(&after, "/src/b.rs", false), "{after:#}");
}

/// A pattern selected from a subdirectory by a spelling that, from the
/// project root, also names a declared path, is written so that the array
/// selects the pattern alone.
#[test]
fn the_json_attest_array_selects_a_pattern_unambiguously() {
	let repo = TestRepo::init();
	repo.write("docs/a.md", "# A\n");
	repo.write("docs/src/a.rs", "fn documented() {}\n");
	repo
		.run_in(
			"docs",
			&[
				"track",
				"a.md",
				"--pattern",
				"src/a.rs",
				"--source",
				"../src/a.rs",
				"--yes",
			],
		)
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo
		.run_in(
			"docs",
			&["diff", "a.md", "--source", "src/a.rs", "--format", "json"],
		)
		.expect_code(0)
		.json();
	assert_eq!(report["attest"][4], "/docs/src/a.rs", "{report:#}");
	replay(&repo, &report).expect_code(0);
	let after = repo
		.run(&["diff", "docs/a.md", "--format", "json"])
		.expect_code(0)
		.json();
	assert!(!changed(&after, "src/a.rs", true), "{after:#}");
	assert!(changed(&after, "../src/a.rs", false), "{after:#}");
}

/// In a nested project, a path that shares its rooted spelling with a
/// pattern is told apart without breaking its `:/` repository root.
#[test]
fn the_json_attest_array_keeps_a_rooted_path_in_a_nested_project() {
	let repo = TestRepo::init();
	repo.write("app/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("app/a.md", "# A\n");
	repo.write("app/code.rs", "fn nested() {}\n");
	repo
		.run_in(
			"app",
			&[
				"track",
				"a.md",
				"--source",
				"/code.rs",
				"--pattern",
				"/code.rs",
				"--yes",
			],
		)
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo
		.run_in(
			"app",
			&["diff", "a.md", "--source", "code.rs", "--format", "json"],
		)
		.expect_code(0)
		.json();
	assert_eq!(report["project"], "app");
	assert_eq!(report["attest"][4], ":/./app/code.rs", "{report:#}");
	replay(&repo, &report).expect_code(0);
	let after = repo
		.run_in("app", &["diff", "a.md", "--format", "json"])
		.expect_code(0)
		.json();
	assert!(!changed(&after, "/code.rs", false), "{after:#}");
	assert!(changed(&after, "/code.rs", true), "{after:#}");
}
