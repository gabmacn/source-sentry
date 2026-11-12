//! Registration inherits series declarations and reports useful commands.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo};

/// A reviewed predecessor in a cross-directory series, followed by a successor.
fn series_repo(on_file: bool, sealed: bool) -> TestRepo {
	let repo = TestRepo::init();
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\ndefine:\n  classes:\n    privacy: {}\n    internal-docs: {}\nseries:\n  audits:\n    pattern: 'audits/(?:[^/]+/)*v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: living\n");
	repo.write("audits/source-sentry.yaml", "root: true\n");
	repo.write("audits/old/v2.md", "# Audit two\n");
	let mut args = vec!["track", "audits/old/v2.md"];
	if on_file {
		args.push("--on-file");
	} else {
		args.extend(["--lockfile", "."]);
	}
	args.extend([
		"--source",
		"src/a.rs",
		"--pattern",
		"src/*.rs",
		"--class",
		"privacy",
		"--owner",
		"legal",
		"--grace",
		"7",
		"--cadence",
		"2026-12-01",
		"FREQ=MONTHLY",
		"--yes",
	]);
	repo.run(&args).expect_code(0);
	repo
		.run(&["attest", "audits/old/v2.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.write("audits/new/deep/v10.md", "# Audit ten\n");
	if sealed {
		repo
			.run(&["seal", "audits/old/v2.md", "--yes"])
			.expect_code(0);
	}
	repo.commit_at("2026-10-02T10:00:00Z");
	repo
}

/// The declaration a root lockfile holds for the successor.
fn successor(repo: &TestRepo, lockfile: &str, key: &str) -> Value {
	let lockfile: Value = serde_json::from_str(&repo.read(lockfile)).unwrap();
	lockfile["documents"][key].clone()
}

/// A sealed predecessor supplies settings and rebased sources, with additions.
#[test]
fn track_inherits_sealed_predecessor_and_shows_the_copy_before_options() {
	let repo = series_repo(false, true);
	repo.write("src/b.txt", "another source\n");
	let args = [
		"track",
		"audits/new/deep/v10.md",
		"--source",
		"/src/b.txt",
		"--class",
		"internal-docs",
		"--owner",
		"platform",
		"--grace",
		"14",
		"--no-cadence",
		"--dry-run",
	];
	let before = repo.read("source-sentry.lock");
	let preview = repo.run(&args);
	preview.expect_code(0);
	for expected in [
		"Inherit from audits/old/v2.md",
		"../../../src/a.rs",
		"../../../src/*.rs",
		"legal",
		"7 days",
		"FREQ=MONTHLY",
		"14 days",
		"Result after command options",
	] {
		assert!(
			preview.stdout.contains(expected),
			"missing {expected}: {}",
			preview.stdout
		);
	}
	assert_eq!(repo.read("source-sentry.lock"), before);
	let mut args = args.to_vec();
	args.pop();
	args.push("--yes");
	repo.run(&args).expect_code(0);
	let entry = successor(&repo, "source-sentry.lock", "audits/new/deep/v10.md");
	assert_eq!(
		entry,
		json!({"classes":["privacy", "internal-docs"], "owners":["legal", "platform"], "schedule":{"grace":14, "cadence":null}, "sources":[{"pattern":"../../../src/*.rs"}, {"path":"../../../src/a.rs"}, {"path":"/src/b.txt"}]})
	);
}

/// On-file declarations may supply or receive inherited settings normally.
#[test]
fn track_inherits_an_on_file_predecessor_into_on_file_placement() {
	let repo = series_repo(true, false);
	let before = repo.read("audits/old/v2.md");
	repo
		.run(&["track", "audits/new/deep/v10.md", "--on-file", "--yes"])
		.expect_code(0);
	let contents = repo.read("audits/new/deep/v10.md");
	for expected in [
		"privacy",
		"legal",
		"../../../src/a.rs",
		"../../../src/*.rs",
		"grace: 7",
	] {
		assert!(contents.contains(expected), "{contents}");
	}
	for absent in ["reviewed:", "completed:", "seal:", "oid:", "members:"] {
		assert!(!contents.contains(absent), "{contents}");
	}
	assert_eq!(repo.read("audits/old/v2.md"), before);
}

/// Both adoption modes inherit into the normal lockfile, including from on-file.
#[test]
fn init_inherits_with_paths_and_all() {
	for all in [false, true] {
		let repo = series_repo(true, false);
		let run = if all {
			repo.run(&["init", "--all", "--owner", "platform", "--yes"])
		} else {
			repo.run(&[
				"init",
				"audits/new/deep/v10.md",
				"--owner",
				"platform",
				"--yes",
			])
		};
		run.expect_code(0);
		assert!(
			run.stdout.contains("Inherit from audits/old/v2.md"),
			"{}",
			run.stdout
		);
		let entry =
			successor(&repo, "audits/source-sentry.lock", "new/deep/v10.md");
		assert_eq!(entry["owners"], json!(["legal", "platform"]));
		assert_eq!(
			entry["sources"],
			json!([{"pattern":"../../../src/*.rs"}, {"path":"../../../src/a.rs"}])
		);
		for absent in ["reviewed", "completed", "seal"] {
			assert!(entry.get(absent).is_none(), "{entry}");
		}
	}
}

/// Human checks, explanations and JSON agree on the inheritance predecessor.
#[test]
fn check_and_explain_hint_at_registration() {
	let repo = series_repo(false, true);
	let human = repo.run(&["check", "--at", AT]);
	assert!(
		human
			.stdout
			.contains("source-sentry track audits/new/deep/v10.md"),
		"{}",
		human.stdout
	);
	assert!(
		human
			.stdout
			.contains("inherit the declaration of `audits/old/v2.md`"),
		"{}",
		human.stdout
	);
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let subject = report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject["path"] == "audits/new/deep/v10.md")
		.unwrap();
	assert_eq!(subject["inherit-from"], "audits/old/v2.md");
	let explained =
		repo.run_in("audits", &["explain", "new/deep/v10.md", "--at", AT]);
	explained.expect_code(0);
	assert!(
		explained
			.stdout
			.contains("source-sentry track new/deep/v10.md"),
		"{}",
		explained.stdout
	);
	assert!(
		explained
			.stdout
			.contains("inherit the declaration of `old/v2.md`"),
		"{}",
		explained.stdout
	);
}

/// Inheritance commands retain the repository and run where they are printed.
#[test]
fn inheritance_commands_preserve_repository_selection() {
	let repo = series_repo(false, true);
	for (directory, repository, document, predecessor) in [
		("audits", "..", "audits/new/deep/v10.md", "audits/old/v2.md"),
		("..", "repo", "audits/new/deep/v10.md", "audits/old/v2.md"),
	] {
		for command in ["check", "explain"] {
			let mut args = vec!["-C", repository, command];
			if command == "explain" {
				args.push(document);
			}
			args.extend(["--at", AT]);
			let run = repo.run_in(directory, &args);
			let hint = run.stdout.split("Run `").nth(1).unwrap();
			let (line, detail) = hint.split_once("` to inherit").unwrap();
			assert_eq!(
				line,
				format!("source-sentry -C {repository} track {document}")
			);
			assert!(detail.contains(&format!("`{predecessor}`")), "{detail}");
			let plan = repo.run_shell_in(directory, &format!("{line} --dry-run"));
			plan.expect_code(0);
			assert!(
				plan.stdout.contains(&format!("Inherit from {predecessor}")),
				"{}",
				plan.stdout
			);
		}
	}
}
