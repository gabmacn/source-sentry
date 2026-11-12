//! Nested projects exercised through the executable and real git snapshots.

use serde_json::Value;

use crate::support::{AT, TestRepo};

/// A nested project with a local document, source and lockfile scan root.
fn nested() -> TestRepo {
	let repo = TestRepo::init();
	repo.write(
		"source-sentry.lock",
		"{\"schema\":1,\"documents\":{},\"retired\":{}}",
	);
	repo.write(
		"app/.source-sentry/source-sentry.yaml",
		"version: 1\nschedule: {grace: 5}\n",
	);
	repo.write(
		"app/source-sentry.lock",
		"{\"schema\":1,\"documents\":{},\"retired\":{}}",
	);
	repo.write("app/guide.md", "# Nested guide\n");
	repo.write("app/code.rs", "fn nested() {}\n");
	repo
}

/// `check` and `init` cover one project, and `-C` changes relative paths.
#[test]
fn check_and_init_stay_in_the_starting_project() {
	let repo = nested();
	let parent = repo.run(&["check", "--at", AT]);
	assert!(
		parent.stdout.contains("Nested projects not checked: app"),
		"{}",
		parent.stdout
	);
	let child = repo
		.run(&["-C", "app", "check", "--at", AT, "--format", "json"])
		.json();
	let subjects = child["subjects"].as_array().unwrap();
	assert_eq!(subjects.len(), 1);
	assert_eq!(subjects[0]["project"], "app");
	assert_eq!(subjects[0]["path"], "guide.md");
	let preview = repo.run(&["-C", "app", "init"]);
	preview.expect_code(0);
	assert!(preview.stdout.contains("guide.md"));
	assert!(!preview.stdout.contains("docs/guide.md"));
	repo.run(&["--repo", ".", "check"]).expect_code(64);
}

/// Document commands select the target's project from outside it.
#[test]
fn nested_document_commands_and_both_source_roots() {
	let repo = nested();
	repo
		.run(&[
			"track",
			"app/guide.md",
			"--source",
			"/code.rs",
			"--source",
			":/src/a.rs",
			"--pattern",
			"/code*.rs",
			"--yes",
		])
		.expect_code(0);
	repo.git(&["add", "app/source-sentry.lock"]);
	let diff = repo.run(&["diff", "app/guide.md", "--format", "json"]);
	diff.expect_code(0);
	let json = diff.json();
	assert_eq!(json["project"], "app");
	assert_eq!(json["document"], "guide.md");
	assert_eq!(json["sources"][0]["entries"][0]["path"], "code.rs");
	repo
		.run(&["attest", "app/guide.md", "--yes"])
		.expect_code(0);
	repo.git(&["add", "app/source-sentry.lock"]);
	repo
		.run(&["explain", "app/guide.md", "--at", AT])
		.expect_code(0);
	repo
		.run(&["untrack", "app/guide.md", "--source", "/code.rs", "--yes"])
		.expect_code(0);
	let lock: Value =
		serde_json::from_str(&repo.read("app/source-sentry.lock")).unwrap();
	assert_eq!(
		lock["documents"]["guide.md"]["sources"]
			.as_array()
			.unwrap()
			.len(),
		2
	);
	for command in ["seal", "retire"] {
		repo
			.run(&[command, "app/guide.md", "--dry-run"])
			.expect_code(0);
	}
}

/// Foreign source freshness uses its declaration even when its context differs.
#[test]
fn cross_project_source_documents_propagate_freshness() {
	let repo = nested();
	repo
		.run(&["track", "app/guide.md", "--source", "/code.rs", "--yes"])
		.expect_code(0);
	repo.git(&["add", "app/source-sentry.lock"]);
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/app/guide.md",
			"--yes",
		])
		.expect_code(0);
	repo.git(&["add", "source-sentry.lock"]);
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.git(&["add", "source-sentry.lock"]);
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	let json = run.json();
	let guide = json["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject["document"] == "docs/guide.md")
		.unwrap();
	assert!(
		guide["facts"]
			.as_array()
			.unwrap()
			.contains(&Value::String("upstream-stale".to_owned())),
		"{json:#}"
	);
	assert!(
		json["subjects"]
			.as_array()
			.unwrap()
			.iter()
			.all(|subject| subject["project"] == ".")
	);
}

/// Invalid nested configs report their own project and relative diagnostic file.
#[test]
fn diagnostics_carry_project_locations() {
	let repo = nested();
	repo.write(
		"app/.source-sentry/source-sentry.yaml",
		"version: 1\nunknown: true\n",
	);
	let json = repo
		.run(&["-C", "app", "check", "--format", "json"])
		.expect_code(3)
		.json();
	assert_eq!(json["diagnostics"][0]["location"]["project"], "app");
	assert_eq!(
		json["diagnostics"][0]["location"]["file"],
		".source-sentry/source-sentry.yaml"
	);
}
