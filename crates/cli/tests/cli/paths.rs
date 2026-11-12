//! Paths typed and printed from the root and from subdirectories.

use serde_json::Value;

use crate::support::{AT, TestRepo};

/// The guide's source declarations in its local lockfile.
fn sources(repo: &TestRepo) -> Vec<Value> {
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	lockfile["documents"]["guide.md"]["sources"]
		.as_array()
		.cloned()
		.unwrap_or_default()
}

/// Runs the last, copyable command in a human diff from the same directory.
fn run_printed_attest(repo: &TestRepo, directory: &str, stdout: &str) {
	let line = stdout.lines().last().unwrap().trim();
	assert!(line.starts_with("source-sentry "), "{line}");
	repo
		.run_shell_in(directory, &format!("{line} --yes"))
		.expect_code(0);
}

/// Native absolute document paths work independently of the current directory.
#[test]
fn documents_accept_absolute_filesystem_paths() {
	let repo = TestRepo::init();
	let document = repo.path().join("docs/guide.md");
	assert!(document.is_absolute());
	let document = document.to_str().unwrap();
	repo
		.run_in(
			"src",
			&["track", document, "--source", "/src/a.rs", "--yes"],
		)
		.expect_code(0);
	for command in ["diff", "explain"] {
		repo.run_in("src", &[command, document]).expect_code(0);
	}
	repo
		.run_in("src", &["attest", document, "--yes"])
		.expect_code(0);
	for command in ["untrack", "seal", "retire"] {
		let mut args = vec![command, document, "--dry-run"];
		if command == "untrack" {
			args.extend(["--source", "/src/a.rs"]);
		}
		repo.run_in("src", &args).expect_code(0);
	}
	assert_eq!(sources(&repo)[0]["path"], "/src/a.rs");
}

/// An absolute repository location works from outside any repository.
#[test]
fn repo_accepts_an_absolute_filesystem_path() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	assert!(repo.path().is_absolute());
	let repository = repo.path().to_str().unwrap();
	let diff = repo.run_in(
		"..",
		&[
			"-C",
			repository,
			"diff",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
		],
	);
	diff.expect_code(0);
	assert!(diff.stdout.contains("docs/guide.md"), "{}", diff.stdout);
	run_printed_attest(&repo, "..", &diff.stdout);
}

/// A source argument starts in the current directory and is stored from
/// the document's directory; rooted spellings remain intact.
#[test]
fn track_reexpresses_paths_and_patterns() {
	for (directory, document, source, pattern) in [
		(".", "docs/guide.md", "src/a.rs", "src/**/*.rs"),
		("docs", "guide.md", "../src/a.rs", "../src/**/*.rs"),
		("src", "../docs/guide.md", "a.rs", "**/*.rs"),
	] {
		let repo = TestRepo::init();
		repo
			.run_in(
				directory,
				&[
					"track",
					document,
					"--source",
					source,
					"--pattern",
					pattern,
					"--yes",
				],
			)
			.expect_code(0);
		let sources = sources(&repo);
		assert!(
			sources.iter().any(|source| source["path"] == "../src/a.rs"),
			"{sources:#?}"
		);
		assert!(
			sources
				.iter()
				.any(|source| source["pattern"] == "../src/**/*.rs"),
			"{sources:#?}"
		);
	}
	let repo = TestRepo::init();
	repo
		.run_in(
			"src",
			&[
				"track",
				"../docs/guide.md",
				"--source",
				"/src/./a.rs",
				"--pattern",
				"/src/*.rs",
				"--yes",
			],
		)
		.expect_code(0);
	assert_eq!(sources(&repo)[1]["path"], "/src/./a.rs");
	assert_eq!(sources(&repo)[0]["pattern"], "/src/*.rs");
}

/// Remote documents receive rooted declarations for current-directory input.
#[test]
fn remote_sources_are_resolved_from_the_current_directory() {
	let repo = TestRepo::init();
	repo
		.run_in(
			"src",
			&[
				"track",
				"abstract:guide",
				"--remote",
				"team",
				"--source",
				"a.rs",
				"--pattern",
				"*.rs",
				"--yes",
			],
		)
		.expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read(".source-sentry/remotes/team.lock"))
			.unwrap();
	assert_eq!(
		lockfile["documents"]["abstract:guide"]["sources"][1]["path"],
		"/src/a.rs"
	);
	assert_eq!(
		lockfile["documents"]["abstract:guide"]["sources"][0]["pattern"],
		"/src/*.rs"
	);
}

/// Unsafe glob rebasing is invalid usage and explains its cause.
#[test]
fn unsafe_patterns_are_usage_errors() {
	let repo = TestRepo::init();
	for (pattern, reason) in [
		("", "the pattern is empty"),
		("src/*/../a.rs", "`..` follows a wildcard"),
		("src/{a,b", "invalid glob syntax"),
	] {
		let run =
			repo.run(&["track", "docs/guide.md", "--pattern", pattern, "--yes"]);
		run.expect_code(64);
		assert!(run.stderr.contains(reason), "{}", run.stderr);
		assert!(!repo.exists("docs/source-sentry.lock"));
	}
}

/// All path selectors refer to the file they name from the current directory.
#[test]
fn selectors_match_resolved_files() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--source", "src/a.rs", "--yes"])
		.expect_code(0);
	for (directory, document, source) in [
		(".", "docs/guide.md", "src/./a.rs"),
		("docs", "guide.md", "../src/a.rs"),
		("src", "../docs/guide.md", "a.rs"),
		("src", "../docs/guide.md", "/src/a.rs"),
	] {
		repo
			.run_in(directory, &["diff", document, "--source", source])
			.expect_code(0);
		repo
			.run_in(
				directory,
				&["attest", document, "--source", source, "--dry-run"],
			)
			.expect_code(0);
	}
	repo
		.run_in(
			"src",
			&["untrack", "../docs/guide.md", "--source", "a.rs", "--yes"],
		)
		.expect_code(0);
	assert_eq!(sources(&repo), [] as [Value; 0]);
}

/// Pattern selectors retain declared and rooted spellings, including removal.
#[test]
fn patterns_are_selected_as_declared_or_rooted() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--pattern", "src/*.rs", "--yes"])
		.expect_code(0);
	for selector in ["../src/*.rs", "/src/*.rs"] {
		let diff =
			repo.run_in("src", &["diff", "../docs/guide.md", "--source", selector]);
		diff.expect_code(0);
		run_printed_attest(&repo, "src", &diff.stdout);
	}
	repo
		.run_in(
			"src",
			&[
				"untrack",
				"../docs/guide.md",
				"--pattern",
				"/src/*.rs",
				"--yes",
			],
		)
		.expect_code(0);
	assert_eq!(sources(&repo), [] as [Value; 0]);
}

/// A selector that names both kinds is refused with both declarations named.
#[test]
fn an_ambiguous_path_and_pattern_are_never_guessed() {
	let repo = TestRepo::init();
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--pattern",
			"src/a.rs",
			"--yes",
		])
		.expect_code(0);
	for command in ["diff", "attest"] {
		let run =
			repo.run_in("docs", &[command, "guide.md", "--source", "../src/a.rs"]);
		run.expect_code(64);
		assert!(run.stderr.contains("path `/src/a.rs`"), "{}", run.stderr);
		assert!(
			run.stderr.contains("pattern `../src/a.rs`"),
			"{}",
			run.stderr
		);
	}
	// At the root this spelling selects only the declared pattern. Its kind
	// must remain explicit when the core receives the selection.
	let run = repo.run(&[
		"diff",
		"docs/guide.md",
		"--source",
		"../src/a.rs",
		"--format",
		"json",
	]);
	run.expect_code(0);
	assert_eq!(run.json()["sources"].as_array().unwrap().len(), 1);
	let json = repo.run_in(
		"src",
		&[
			"diff",
			"../docs/guide.md",
			"--source",
			"./a.rs",
			"--format",
			"json",
		],
	);
	json.expect_code(0);
	let report = json.json();
	let mut command: Vec<_> = report["attest"]
		.as_array()
		.unwrap()
		.iter()
		.skip(1)
		.map(|word| word.as_str().unwrap())
		.collect();
	command.push("--yes");
	repo.run(&command).expect_code(0);
}

/// Copyable commands preserve quoted filenames and values beginning with `-`.
#[test]
fn printed_commands_quote_and_protect_path_arguments() {
	for (document, source, selector) in [
		("docs/a guide.md", "src/a file@name.rs", "a file@name.rs"),
		("docs/-guide.md", "src/-a.rs", "-a.rs"),
	] {
		let repo = TestRepo::init();
		repo.write(document, "# Guide\n");
		repo.write(source, "fn a() {}\n");
		repo
			.run(&["track", document, "--source", source, "--yes"])
			.expect_code(0);
		let diff = repo.run_in(
			"src",
			&[
				"diff",
				&format!("../{document}"),
				&format!("--source={selector}"),
			],
		);
		diff.expect_code(0);
		run_printed_attest(&repo, "src", &diff.stdout);
	}
}

/// Both the printed document and expectations work from a subdirectory.
#[test]
fn human_diff_commands_run_where_they_are_printed() {
	for directory in ["docs", "src"] {
		let repo = TestRepo::init();
		repo.reviewed_guide();
		repo.write("src/a.rs", "fn changed() {}\n");
		let run = repo.run_in(
			directory,
			&[
				"diff",
				if directory == "docs" {
					"guide.md"
				} else {
					"../docs/guide.md"
				},
				"--source",
				"/src/a.rs",
			],
		);
		run.expect_code(0);
		let path = if directory == "docs" {
			"../src/a.rs"
		} else {
			"a.rs"
		};
		assert!(
			run.stdout.contains(&format!("{path}@sha1:")),
			"{}",
			run.stdout
		);
		assert!(
			run.stdout.contains(&format!("+++ {path} (")),
			"{}",
			run.stdout
		);
		run_printed_attest(&repo, directory, &run.stdout);
		repo
			.run_in(directory, &["check", "--at", AT])
			.expect_code(0);
	}
}

/// A deleted pattern member retains a current-directory absence expectation.
#[test]
fn printed_absence_expectations_work_from_a_subdirectory() {
	let repo = TestRepo::init();
	repo.write("src/b.rs", "fn b() {}\n");
	repo
		.run(&["track", "docs/guide.md", "--pattern", "src/*.rs", "--yes"])
		.expect_code(0);
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-03T10:00:00Z");
	repo.git(&["rm", "--quiet", "src/b.rs"]);
	let run = repo.run_in("docs", &["diff", "guide.md"]);
	run.expect_code(0);
	assert!(run.stdout.contains("../src/b.rs@absent"), "{}", run.stdout);
	run_printed_attest(&repo, "docs", &run.stdout);
}

/// JSON keeps repository paths and emits commands to run from the root.
#[test]
fn json_paths_are_independent_of_the_current_directory() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn changed() {}\n");
	let root = repo
		.run(&["diff", "docs/guide.md", "--format", "json"])
		.json();
	let nested = repo
		.run_in(
			"src",
			&["-C", "..", "diff", "docs/guide.md", "--format", "json"],
		)
		.json();
	assert_eq!(root, nested);
	assert_eq!(nested["document"], "docs/guide.md");
	assert_eq!(nested["sources"][0]["entries"][0]["path"], "src/a.rs");
	assert_eq!(nested["attest"][2], "docs/guide.md");
	let mut command: Vec<_> = nested["attest"]
		.as_array()
		.unwrap()
		.iter()
		.skip(1)
		.map(|word| word.as_str().unwrap())
		.collect();
	command.push("--yes");
	repo.run(&command).expect_code(0);
	assert_eq!(
		repo.run(&["check", "--at", AT, "--format", "json"]).json(),
		repo
			.run_in("src", &["check", "--at", AT, "--format", "json"])
			.json()
	);
}

/// JSON commands protect option-like documents and retain rooted selectors.
#[test]
fn json_commands_run_from_the_root_with_protected_document_names() {
	for (document, expected) in [
		("./-guide.md", "./-guide.md"),
		("docs/a guide.md", "docs/a guide.md"),
	] {
		let repo = TestRepo::init();
		repo.write(document, "# Guide\n");
		repo
			.run(&["track", document, "--source", "src/a.rs", "--yes"])
			.expect_code(0);
		let report = repo
			.run_in(
				"src",
				&[
					"-C", "..", "diff", document, "--source", "src/a.rs", "--format",
					"json",
				],
			)
			.expect_code(0)
			.json();
		assert_eq!(report["attest"][2], expected);
		assert_eq!(report["attest"][4], "/src/a.rs");
		assert!(
			report["attest"][6]
				.as_str()
				.unwrap()
				.starts_with("/src/a.rs@")
		);
		let mut command: Vec<_> = report["attest"]
			.as_array()
			.unwrap()
			.iter()
			.skip(1)
			.map(|word| word.as_str().unwrap())
			.collect();
		command.push("--yes");
		repo.run(&command).expect_code(0);
	}
}

/// Expectations allow parent components and split the state at the last `@`.
#[test]
fn expectations_resolve_current_and_rooted_paths() {
	let repo = TestRepo::init();
	repo.write("src/a@b.rs", "fn b() {}\n");
	repo
		.run(&["track", "docs/guide.md", "--source", "src/a@b.rs", "--yes"])
		.expect_code(0);
	let oid = repo.git(&["hash-object", "src/a@b.rs"]);
	for path in ["../src/./a@b.rs", "/src/a@b.rs"] {
		repo
			.run_in(
				"docs",
				&[
					"attest",
					"guide.md",
					"--expect",
					&format!("{path}@sha1:{oid}"),
					"--yes",
				],
			)
			.expect_code(0);
	}
}

/// Placement directories, init candidates and documents use filesystem paths.
#[test]
fn placements_and_init_follow_the_same_path_rule() {
	for absolute in [false, true] {
		let repo = TestRepo::init();
		let directory = if absolute {
			repo.path().join("docs").to_str().unwrap().to_owned()
		} else {
			"../docs".to_owned()
		};
		repo
			.run_in(
				"src",
				&[
					"track",
					"../docs/guide.md",
					"--lockfile",
					&directory,
					"--yes",
				],
			)
			.expect_code(0);
		let moved = repo.run_in(
			"src",
			&[
				"track",
				"../docs/guide.md",
				"--from",
				&format!("lockfile:{directory}"),
				"--to",
				"on-file",
				"--yes",
			],
		);
		moved.expect_code(0);
		assert!(
			moved.stdout.contains("Move ../docs/guide.md"),
			"{}",
			moved.stdout
		);
		repo
			.run_in(
				"docs",
				&[
					"track",
					"guide.md",
					"--from",
					"on-file",
					"--to",
					&format!("lockfile:{directory}"),
					"--yes",
				],
			)
			.expect_code(0);
		repo.write("docs/other.md", "# Other\n");
		let candidate = if absolute {
			repo
				.path()
				.join("docs/other.md")
				.to_str()
				.unwrap()
				.to_owned()
		} else {
			"other.md".to_owned()
		};
		let init = repo.run_in("docs", &["init", &candidate, "--yes"]);
		init.expect_code(0);
		assert!(init.stdout.contains("Track other.md"), "{}", init.stdout);
		assert!(
			init.stdout.contains("write   source-sentry.lock"),
			"{}",
			init.stdout
		);
		repo
			.run_in("docs", &["-C", "..", "explain", "docs/guide.md"])
			.expect_code(0);
	}
}

/// Subjects, source paths, origins and diagnostic locations are clickable.
#[test]
fn human_reports_use_the_current_directory() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn changed() {}\n");
	let check = repo.run_in("docs", &["check", "--at", AT]);
	check.expect_code(1);
	assert!(check.stdout.contains("guide.md"), "{}", check.stdout);
	assert!(!check.stdout.contains("docs/guide.md"), "{}", check.stdout);
	assert!(
		check.stdout.contains("../src/a.rs changed"),
		"{}",
		check.stdout
	);
	let explain = repo.run_in("docs", &["explain", "guide.md", "--at", AT]);
	explain.expect_code(0);
	assert!(
		explain.stdout.contains("in source-sentry.lock"),
		"{}",
		explain.stdout
	);
	assert!(explain.stdout.contains("../src/a.rs"), "{}", explain.stdout);
	repo.commit_at("2026-10-03T10:00:00Z");
	repo.git(&["rm", "--quiet", "src/a.rs"]);
	let check = repo.run_in("docs", &["check", "--at", AT]);
	check.expect_code(3);
	assert!(
		check.stdout.contains("source-sentry.lock:"),
		"{}",
		check.stdout
	);
	assert!(
		!check.stdout.contains("docs/source-sentry.lock:"),
		"{}",
		check.stdout
	);
}

/// Configuration failures and quiet diagnostics display local locations.
#[test]
fn failure_locations_use_the_current_directory() {
	let repo = TestRepo::init();
	repo.write("docs/source-sentry.yaml", "schedule:\n  grace: wrong\n");
	let run = repo.run_in("docs", &["check"]);
	run.expect_code(3);
	assert!(
		run.stderr.contains("source-sentry.yaml:2"),
		"{}",
		run.stderr
	);
	assert!(
		!run.stderr.contains("docs/source-sentry.yaml:"),
		"{}",
		run.stderr
	);
	let json = repo.run_in("docs", &["check", "--format", "json"]);
	json.expect_code(3);
	assert_eq!(
		json.json()["diagnostics"][0]["location"]["file"],
		"docs/source-sentry.yaml"
	);
}

/// Changing directory is preserved by pasted hints.
#[test]
fn printed_commands_preserve_repository_selection() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn changed() {}\n");
	let diff = repo.run_in("src", &["-C", "..", "diff", "docs/guide.md"]);
	diff.expect_code(0);
	run_printed_attest(&repo, "src", &diff.stdout);
	repo.write("docs/new.md", "# New\n");
	let preview = repo.run_in("..", &["-C", "repo", "init"]);
	preview.expect_code(0);
	assert!(
		preview.stdout.contains("source-sentry -C repo init --all"),
		"{}",
		preview.stdout
	);
}
