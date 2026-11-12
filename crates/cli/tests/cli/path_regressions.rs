//! Lexical source identities, exact rename spellings and copyable path hints.

use serde_json::Value;

use crate::support::{AT, TestRepo};

/// Relative and rooted source spellings track the git symlink itself.
#[test]
fn relative_source_paths_never_follow_symlinks() {
	let repo = TestRepo::init();
	repo.git(&["config", "core.symlinks", "true"]);
	repo.write("src/other.rs", "fn other() {}\n");
	let link = repo.path().join("src/link.rs");
	#[cfg(unix)]
	std::os::unix::fs::symlink("a.rs", &link).unwrap();
	#[cfg(windows)]
	std::os::windows::fs::symlink_file("a.rs", &link).unwrap();
	repo.git(&["add", "src/link.rs"]);
	repo.write("docs/rooted.md", "# Rooted\n");
	for (document, source) in [
		("docs/guide.md", "src/link.rs"),
		("docs/rooted.md", "/src/link.rs"),
	] {
		repo
			.run(&["track", document, "--source", source, "--yes"])
			.expect_code(0);
		repo.run(&["attest", document, "--yes"]).expect_code(0);
	}
	repo.commit_at("2026-10-01T12:00:00Z");
	let lock: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lock["documents"]["guide.md"]["sources"][0]["path"],
		"../src/link.rs"
	);
	std::fs::remove_file(&link).unwrap();
	#[cfg(unix)]
	std::os::unix::fs::symlink("other.rs", &link).unwrap();
	#[cfg(windows)]
	std::os::windows::fs::symlink_file("other.rs", &link).unwrap();
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	for subject in report["subjects"].as_array().unwrap() {
		if subject["kind"] == "document" {
			assert_eq!(subject["sources"][0]["state"], "drifted");
		}
	}
}

/// Case-only moves retain the exact old and new git spellings on every
/// platform, including when filesystem existence ignores case.
#[test]
fn case_only_renames_keep_git_spellings() {
	for on_file in [false, true] {
		let repo = TestRepo::init();
		let placement = if on_file { "--on-file" } else { "--lockfile" };
		repo
			.run(&[
				"track",
				"docs/guide.md",
				placement,
				"--source",
				"/src/a.rs",
				"--yes",
			])
			.expect_code(0);
		repo
			.run(&["attest", "docs/guide.md", "--yes"])
			.expect_code(0);
		repo.commit_at("2026-10-01T12:00:00Z");
		repo.git(&["mv", "docs/guide.md", "docs/Guide.md"]);
		let plan =
			repo.run_in("docs", &["rename", "guide.md", "Guide.md", "--dry-run"]);
		plan.expect_code(0);
		assert!(
			plan.stdout.contains("Rename guide.md to Guide.md"),
			"{}",
			plan.stdout
		);
		repo
			.run(&["rename", "docs/guide.md", "docs/Guide.md", "--yes"])
			.expect_code(0);
		repo.run(&["check", "--at", AT]).expect_code(0);
		if !on_file {
			let lock: Value =
				serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
			assert!(lock["documents"].get("guide.md").is_none());
			assert_eq!(
				lock["documents"]["Guide.md"]["renamed"][0]["from"],
				"/docs/guide.md"
			);
		}
	}
}

/// The temporary old/new pair may share a series version; a third entry
/// still refuses the final inventory without changing the declaration.
#[test]
fn same_version_series_renames_validate_the_final_inventory() {
	let repo = TestRepo::init();
	repo.git(&["rm", "-f", "docs/guide.md"]);
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\nseries:\n  audits:\n    pattern: 'records/(?:moved/|other/)?audit-(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: records-only\n");
	repo.write("records/audit-1.md", "# Audit\n");
	repo
		.run(&["track", "records/audit-1.md", "--lockfile", ".", "--yes"])
		.expect_code(0);
	repo
		.run(&["seal", "records/audit-1.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T12:00:00Z");
	let before: Value =
		serde_json::from_str(&repo.read("source-sentry.lock")).unwrap();
	std::fs::create_dir_all(repo.path().join("records/moved")).unwrap();
	repo.git(&["mv", "records/audit-1.md", "records/moved/audit-1.md"]);
	repo.write("records/other/audit-1.md", "# Competing entry\n");
	let unchanged = repo.read("source-sentry.lock");
	repo
		.run(&[
			"rename",
			"records/audit-1.md",
			"records/moved/audit-1.md",
			"--yes",
		])
		.expect_code(3);
	assert_eq!(repo.read("source-sentry.lock"), unchanged);
	repo.git(&["rm", "-f", "records/other/audit-1.md"]);
	repo
		.run(&[
			"rename",
			"records/audit-1.md",
			"records/moved/audit-1.md",
			"--yes",
		])
		.expect_code(0);
	let after: Value =
		serde_json::from_str(&repo.read("source-sentry.lock")).unwrap();
	assert_eq!(
		after["documents"]["records/moved/audit-1.md"]["seal"],
		before["documents"]["records/audit-1.md"]["seal"]
	);
	repo.run(&["check", "--at", AT]).expect_code(0);
}

/// A renamed live completion must not borrow an identical marker from a
/// retired incarnation of its former path. Retirement afterwards keeps it.
#[test]
fn renamed_completion_credit_keeps_its_incarnation() {
	let repo = TestRepo::init();
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\nseries:\n  audits:\n    pattern: 'records/audit-(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: records-only\n    schedule:\n      cadence:\n        dtstart: 2026-01-01\n        rrule: FREQ=DAILY\n");
	repo.write("records/audit-1.md", "# Audit\n");
	let oid = repo.git(&["hash-object", "records/audit-1.md"]);
	let seal = serde_json::json!({"at": "2026-01-01T00:00:00Z", "by": "Recorder", "oid": format!("sha1:{oid}")});
	let retired = serde_json::json!({"seal": seal, "retired": {"at": "2026-01-01T00:00:00Z", "by": null}});
	let mut lock = serde_json::json!({"schema": 1, "documents": {}, "retired": {"records/audit-1.md": [retired]}});
	repo.write("source-sentry.lock", &lock.to_string());
	repo.commit_at("2026-01-01T12:00:00Z");
	lock["documents"]["records/audit-1.md"] = serde_json::json!({"seal": seal});
	repo.write("source-sentry.lock", &lock.to_string());
	repo.commit_at("2026-01-02T12:00:00Z");
	let series = |report: Value| {
		report["subjects"]
			.as_array()
			.unwrap()
			.iter()
			.find(|subject| subject["kind"] == "series")
			.unwrap()
			.clone()
	};
	let check = || {
		series(
			repo
				.run(&["check", "--format", "json", "--at", "2026-01-05T12:00:00Z"])
				.json(),
		)
	};
	let before = check();
	repo.git(&["mv", "records/audit-1.md", "records/audit-2.md"]);
	repo
		.run(&[
			"rename",
			"records/audit-1.md",
			"records/audit-2.md",
			"--yes",
		])
		.expect_code(0);
	for committed in [false, true] {
		if committed {
			repo.commit_at("2026-01-03T12:00:00Z");
		}
		let after = check();
		assert_eq!(after["missed"], before["missed"]);
	}
	repo
		.run(&["retire", "records/audit-2.md", "--yes"])
		.expect_code(0);
	repo.git(&["rm", "records/audit-2.md"]);
	repo.commit_at("2026-01-04T12:00:00Z");
	assert_eq!(check()["missed"], before["missed"]);
}

/// Refusals and nested expectation mismatches use the effective directory,
/// while quoted source declarations keep their written spelling.
#[test]
fn refusals_and_expectation_mismatches_display_local_paths() {
	let repo = TestRepo::init();
	repo.write("docs/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo
		.run(&["track", "docs/guide.md", "--source", "src/a.rs", "--yes"])
		.expect_code(0);
	for args in [
		vec!["diff", "missing.md"],
		vec!["track", "missing.md", "--dry-run"],
		vec!["diff", "guide.md", "--source", "../src/missing.rs"],
		vec!["track", "guide.md", "--source", "../src/a.rs", "--dry-run"],
	] {
		let run = repo.run_in("docs", &args);
		run.expect_code(2);
		assert!(!run.stderr.contains("`docs/"), "{}", run.stderr);
	}
	let mismatch = repo.run_in(
		"docs",
		&[
			"-C",
			".",
			"attest",
			"guide.md",
			"--expect",
			"../src/a.rs@sha1:0000000000000000000000000000000000000000",
			"--dry-run",
		],
	);
	mismatch.expect_code(2);
	assert!(
		mismatch.stderr.contains("for `guide.md`"),
		"{}",
		mismatch.stderr
	);
	assert!(
		mismatch.stderr.contains("expected `../src/a.rs@"),
		"{}",
		mismatch.stderr
	);
	assert!(
		mismatch.stderr.contains("found `../src/a.rs@"),
		"{}",
		mismatch.stderr
	);
	let extra = repo.run_in(
		"docs",
		&[
			"attest",
			"guide.md",
			"--expect",
			"../src/other.rs@absent",
			"--dry-run",
		],
	);
	extra.expect_code(2);
	assert!(extra.stderr.contains("`../src/a.rs@"), "{}", extra.stderr);
	assert!(
		extra
			.stderr
			.contains("`../src/other.rs` is not part of the selection"),
		"{}",
		extra.stderr
	);
	let repeated = repo.run_in(
		"docs",
		&[
			"attest",
			"guide.md",
			"--expect",
			"../src/a.rs@absent",
			"--expect",
			"../src/a.rs@absent",
			"--dry-run",
		],
	);
	repeated.expect_code(2);
	assert!(
		repeated
			.stderr
			.contains("`../src/a.rs` is named more than once"),
		"{}",
		repeated.stderr
	);
}

/// A committed POSIX filename that resembles a remote identity receives a
/// local argument prefix, even when the host cannot check out that filename.
#[test]
fn series_hints_protect_local_identity_like_filenames() {
	let repo = TestRepo::init();
	let config = repo.git_input(&["hash-object", "-w", "--stdin"], "version: 1\nseries:\n  proposals:\n    pattern: 'abstract:v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: records-only\n");
	let configs = repo.git_input(
		&["mktree"],
		&format!("100644 blob {config}\tsource-sentry.yaml\n"),
	);
	let content = repo.git_input(&["hash-object", "-w", "--stdin"], "# Entry\n");
	let lock = repo.git_input(
		&["hash-object", "-w", "--stdin"],
		r#"{"schema":1,"retired":{},"documents":{"abstract:v1.md":{}}}"#,
	);
	let tree = repo.git_input(&["mktree"], &format!("040000 tree {configs}\t.source-sentry\n100644 blob {content}\tabstract:v1.md\n100644 blob {content}\tabstract:v2.md\n100644 blob {lock}\tsource-sentry.lock\n"));
	let commit =
		repo.git(&["commit-tree", &tree, "-m", "Literal document names"]);
	let run = repo.run(&["check", "--rev", &commit]);
	assert!(
		run.stdout.contains("source-sentry track ./abstract:v2.md"),
		"{}",
		run.stdout
	);
	let replay = repo.run(&["track", "./abstract:v2.md", "--dry-run"]);
	assert_ne!(replay.code, 64, "{}", replay.stderr);
}

/// The path overview assigns pattern options only to supported commands.
#[test]
fn path_overview_uses_source_for_review_pattern_selectors() {
	let reference = include_str!("../../../../docs/cli.md");
	assert!(reference.contains(
		"`diff` and `attest` select declared patterns through `--source`"
	));
	let repo = TestRepo::init();
	for command in ["diff", "attest"] {
		repo
			.run(&[command, "docs/guide.md", "--pattern", "/src/*.rs"])
			.expect_code(64);
	}
}
