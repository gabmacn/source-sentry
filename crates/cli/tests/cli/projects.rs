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

/// In frontmatter too, `/` starts at the document's project root and `:/` at
/// the repository's, so repeated sources are found by the files they name.
#[test]
fn on_file_sources_are_compared_in_the_documents_project() {
	let repo = nested();
	repo.write("app/src/a.rs", "fn nested() {}\n");
	let guide = |first: &str, second: &str| {
		format!(
			"---\nsource-sentry:\n  sources:\n    - path: \"{first}\"\n    - path: \
			 \"{second}\"\n---\n# Guide\n"
		)
	};

	repo.write("app/guide.md", &guide("/src/a.rs", ":/src/a.rs"));
	let diff = repo.run(&["-C", "app", "diff", "guide.md", "--format", "json"]);
	diff.expect_code(0);
	let json = diff.json();
	let named: Vec<&str> = json["sources"]
		.as_array()
		.unwrap()
		.iter()
		.map(|source| source["entries"][0]["path"].as_str().unwrap())
		.collect();
	assert_eq!(named, ["src/a.rs", "../src/a.rs"], "{json:#}");
	let check = repo.run(&["-C", "app", "check", "--at", AT]);
	assert!(
		!check.stdout.contains("configuration-error"),
		"{}",
		check.stdout
	);

	repo.write("app/guide.md", &guide("/src/a.rs", "src/a.rs"));
	let check = repo.run(&["-C", "app", "check", "--at", AT]);
	check.expect_code(3);
	assert!(
		check.stdout.contains(
			"`source-sentry.sources[1].path` repeats \
			 `source-sentry.sources[0].path`: both name `app/src/a.rs`"
		),
		"{}",
		check.stdout
	);
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

/// Cross-project renames preserve reviews while rewriting both rooted source kinds.
#[test]
fn rename_across_projects_previews_sources_and_settings() {
	let repo = nested();
	repo.write("app/.source-sentry/source-sentry.yaml", "version: 1\ntime-zone: Europe/Madrid\ndefine:\n  contexts:\n    default:\n      fallback: fail\nschedule: {grace: 5}\ndocuments:\n  adapters:\n    - match: '*.md'\n      adapter: opaque\n");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--pattern",
			"/src/*.rs",
			"--yes",
		])
		.expect_code(0);
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.git(&["mv", "docs/guide.md", "app/moved.md"]);
	let plan =
		repo.run(&["rename", "docs/guide.md", "app/moved.md", "--dry-run"]);
	plan.expect_code(0);
	assert!(
		plan
			.stdout
			.contains("time-zone changes: UTC -> Europe/Madrid"),
		"{}",
		plan.stdout
	);
	assert!(plan.stdout.contains("contexts changes:"), "{}", plan.stdout);
	assert!(
		plan.stdout.contains("adapter changes: md-utf8 -> opaque"),
		"{}",
		plan.stdout
	);
	assert!(
		plan.stdout.contains("`/src/a.rs` -> `:/src/a.rs`"),
		"{}",
		plan.stdout
	);
	assert!(
		plan.stdout.contains("grace changes: 0 days"),
		"{}",
		plan.stdout
	);
	repo
		.run(&["rename", "docs/guide.md", "app/moved.md", "--yes"])
		.expect_code(0);
	let lock: Value =
		serde_json::from_str(&repo.read("app/source-sentry.lock")).unwrap();
	let sources = lock["documents"]["moved.md"]["sources"].as_array().unwrap();
	assert!(sources.iter().any(
		|source| source["path"] == ":/src/a.rs" && source["reviewed"].is_object()
	));
	assert!(
		sources
			.iter()
			.any(|source| source["pattern"] == ":/src/*.rs"
				&& source["members"][0]["path"] == ":/src/a.rs")
	);
}

/// A destination must declare every explicit class used by the moved declaration.
#[test]
fn rename_refuses_a_class_absent_from_the_destination() {
	let repo = nested();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  classes:\n    legal: {}\n",
	);
	repo
		.run(&["track", "docs/guide.md", "--class", "legal", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.git(&["mv", "docs/guide.md", "app/moved.md"]);
	let run = repo.run(&["rename", "docs/guide.md", "app/moved.md", "--dry-run"]);
	run.expect_code(2);
	assert!(
		run
			.stderr
			.contains("destination project app does not define class `legal`"),
		"{}",
		run.stderr
	);
}

/// Adding a marker strands outer declarations until a placement move repairs them.
#[test]
fn track_from_repairs_a_new_project_boundary_and_refuses_outward_placement() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\nschedule: {grace: 9}\n",
	);
	repo.write("app/guide.md", "# Guide\n");
	repo
		.run(&[
			"track",
			"app/guide.md",
			"--lockfile",
			".",
			"--source",
			"/src/a.rs",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.write(
		"app/.source-sentry/source-sentry.yaml",
		"version: 1\nschedule: {grace: 5}\n",
	);
	for (directory, expected) in [
		(
			".",
			"track app/guide.md --from lockfile:. --to lockfile:app",
		),
		("app", "track guide.md --from lockfile:.. --to lockfile:."),
	] {
		let run = repo.run_in(directory, &["check", "--at", AT]);
		run.expect_code(3);
		assert!(run.stdout.contains(expected), "{}", run.stdout);
	}
	let selected = repo.run_in("app", &["-C", "..", "check", "--at", AT]);
	selected.expect_code(3);
	assert!(
		selected.stdout.contains(
			"source-sentry -C .. track app/guide.md --from lockfile:. --to lockfile:app"
		),
		"{}",
		selected.stdout
	);
	let args = [
		"track",
		"app/guide.md",
		"--from",
		"lockfile:.",
		"--to",
		"lockfile:app",
		"--dry-run",
	];
	let plan = repo.run(&args);
	plan.expect_code(0);
	assert!(
		plan.stdout.contains("`/src/a.rs` -> `:/src/a.rs`"),
		"{}",
		plan.stdout
	);
	assert!(
		plan.stdout.contains("grace changes: 9 days"),
		"{}",
		plan.stdout
	);
	let mut apply = args;
	apply[6] = "--yes";
	repo.run(&apply).expect_code(0);
	let run = repo.run(&[
		"track",
		"app/guide.md",
		"--from",
		"lockfile:app",
		"--to",
		"lockfile:.",
		"--dry-run",
	]);
	run.expect_code(2);
	assert!(
		run.stderr.contains("git mv")
			&& run.stderr.contains("source-sentry rename"),
		"{}",
		run.stderr
	);
}

/// Remote owners can move between explicitly selected project roots.
#[test]
fn remote_moves_keep_identity_and_rewrite_project_rooted_sources() {
	let repo = nested();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\nschedule: {grace: 9}\n",
	);
	repo
		.run(&[
			"track",
			"abstract:video",
			"--remote",
			"team",
			"--source",
			"/src/a.rs",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	let plan = repo.run(&[
		"track",
		"abstract:video",
		"--from",
		"remote:team",
		"--to",
		"remote:team@app",
		"--dry-run",
	]);
	plan.expect_code(0);
	assert!(
		plan.stdout.contains("projects: . -> app"),
		"{}",
		plan.stdout
	);
	assert!(
		plan.stdout.contains("`/src/a.rs` -> `:/src/a.rs`"),
		"{}",
		plan.stdout
	);
	repo
		.run(&[
			"track",
			"abstract:video",
			"--from",
			"remote:team",
			"--to",
			"remote:team@app",
			"--yes",
		])
		.expect_code(0);
	let lock: Value =
		serde_json::from_str(&repo.read("app/.source-sentry/remotes/team.lock"))
			.unwrap();
	assert_eq!(
		lock["documents"]["abstract:video"]["sources"][0]["path"],
		":/src/a.rs"
	);
	repo
		.run_in("app", &["diff", "abstract:video"])
		.expect_code(0);
	let invalid = repo.run(&[
		"track",
		"abstract:video",
		"--from",
		"remote:team@app",
		"--to",
		"remote:other@app/docs",
		"--dry-run",
	]);
	invalid.expect_code(2);
	assert!(
		invalid.stderr.contains("nearest project root is `app`"),
		"{}",
		invalid.stderr
	);
}

/// A directory spelling remains available as a series name in the starting project.
#[test]
fn explain_series_names_do_not_select_a_nested_project_directory() {
	let repo = nested();
	repo.write(
		".source-sentry/source-sentry.yaml",
		r"version: 1
series:
  app:
    pattern: 'docs/v(?P<version>\d+)\.md'
    version-kind: integer
    lifecycle: records-only
",
	);
	repo.write("docs/v1.md", "# Version\n");
	repo.commit_at("2026-10-01T10:00:00Z");
	for revision in [false, true] {
		let mut args = vec!["explain", "app", "--at", AT];
		if revision {
			args.extend(["--rev", "HEAD"]);
		}
		let run = repo.run(&args);
		run.expect_code(0);
		assert!(run.stdout.contains("app"), "{}", run.stdout);
	}
}

/// A lockfile entry reviewed on 1 November 2026.
const REVIEWED: &str =
	"{\"reviewed\":{\"at\":\"2026-11-01T12:00:00Z\",\"by\":\"Reviewer\"}}";

/// A lockfile holding `documents`, each reviewed.
fn reviewed_lockfile(documents: &[&str]) -> String {
	let entries: Vec<String> = documents
		.iter()
		.map(|document| format!("\"{document}\":{REVIEWED}"))
		.collect();
	format!(
		"{{\"schema\":1,\"documents\":{{{}}},\"retired\":{{}}}}",
		entries.join(",")
	)
}

/// The lines of `text` that report `outside-authority`.
fn outside_authority_lines(text: &str) -> Vec<&str> {
	text
		.lines()
		.filter(|line| line.contains("outside-authority"))
		.collect()
}

/// A document declared both in its own project and in the project above
/// fails two authority checks that give the same advice: the advice is
/// printed once, and the summary and the JSON report still count both.
#[test]
fn the_same_advice_for_one_declaration_is_printed_once() {
	let repo = nested();
	repo.write("app/source-sentry.lock", &reviewed_lockfile(&["guide.md"]));
	repo.write("source-sentry.lock", &reviewed_lockfile(&["app/guide.md"]));
	repo.git(&["add", "."]);
	repo.commit_at("2026-11-01T12:00:00Z");
	let advice = "configuration-error  outside-authority  \
	              ../source-sentry.lock:1  `guide.md` is declared outside its \
	              project. Run `source-sentry -C app track guide.md --from \
	              lockfile:.. --to lockfile:.` to move the declaration into its \
	              project";

	let run = repo.run(&["-C", "app", "check", "--at", AT]);
	run.expect_code(3);
	let lines = outside_authority_lines(&run.stdout);
	assert_eq!(lines.len(), 1, "{}", run.stdout);
	assert_eq!(lines[0].trim(), advice, "{}", run.stdout);
	assert!(
		run.stdout.contains("· 2 configuration errors ·"),
		"{}",
		run.stdout
	);

	let quiet = repo.run(&["-C", "app", "--quiet", "check", "--at", AT]);
	quiet.expect_code(3);
	assert_eq!(
		outside_authority_lines(&quiet.stderr),
		[advice],
		"{}",
		quiet.stderr
	);

	let explain = repo.run(&["-C", "app", "explain", "guide.md", "--at", AT]);
	explain.expect_code(3);
	assert_eq!(
		outside_authority_lines(&explain.stdout).len(),
		1,
		"{}",
		explain.stdout
	);

	let json = repo
		.run(&["-C", "app", "check", "--at", AT, "--format", "json"])
		.expect_code(3)
		.json();
	let messages: Vec<&Value> = json["diagnostics"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|diagnostic| diagnostic["code"] == "outside-authority")
		.map(|diagnostic| &diagnostic["message"])
		.collect();
	assert_eq!(messages.len(), 2, "{json}");
	assert_eq!(messages[0], messages[1], "{json}");
	assert_eq!(json["summary"]["errors"]["configuration"], 2, "{json}");
}

/// Authority failures that give different advice each keep their line.
#[test]
fn different_advice_is_printed_for_each_declaration() {
	let repo = nested();
	repo.write("app/notes.md", "# Notes\n");
	repo.write(
		"source-sentry.lock",
		&reviewed_lockfile(&["app/guide.md", "app/notes.md"]),
	);
	repo.git(&["add", "."]);
	repo.commit_at("2026-11-01T12:00:00Z");
	let run = repo.run(&["-C", "app", "check", "--at", AT]);
	run.expect_code(3);
	let lines = outside_authority_lines(&run.stdout);
	assert_eq!(lines.len(), 2, "{}", run.stdout);
	assert!(lines[0].contains("track guide.md"), "{}", run.stdout);
	assert!(lines[1].contains("track notes.md"), "{}", run.stdout);
}

/// Of another project, `check` reports the errors that concern the cited
/// document: its configuration errors and the errors met while hashing the
/// cited document's sources. A fault in an unrelated document there neither
/// reaches the report nor changes the exit code.
#[test]
fn a_cited_documents_errors_reach_the_citing_report() {
	let repo = nested();
	repo.write("app/other.md", "# Other\n");
	repo.write("app/other.rs", "fn other() {}\n");
	for (document, source) in
		[("app/guide.md", "/code.rs"), ("app/other.md", "/other.rs")]
	{
		repo
			.run(&["track", document, "--source", source, "--yes"])
			.expect_code(0);
		repo.git(&["add", "app/source-sentry.lock"]);
		repo.run(&["attest", document, "--yes"]).expect_code(0);
		repo.git(&["add", "app/source-sentry.lock"]);
	}
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
	let check = || repo.run(&["check", "--at", AT, "--format", "json"]);
	let codes = |json: &Value| -> Vec<String> {
		json["diagnostics"]
			.as_array()
			.unwrap()
			.iter()
			.map(|diagnostic| diagnostic["code"].as_str().unwrap().to_owned())
			.collect()
	};
	let healthy = check().expect_code(0).json();
	assert_eq!(codes(&healthy), Vec::<String>::new(), "{healthy:#}");

	repo.git(&["config", "filter.custom.clean", "cat"]);
	repo.write(".gitattributes", "app/other.rs filter=custom\n");
	let direct = repo
		.run(&["-C", "app", "check", "--at", AT, "--format", "json"])
		.expect_code(4)
		.json();
	assert_eq!(codes(&direct), ["unsupported-filter"], "{direct:#}");
	let unrelated = check().expect_code(0).json();
	assert_eq!(codes(&unrelated), Vec::<String>::new(), "{unrelated:#}");

	repo.write(".gitattributes", "app/code.rs filter=custom\n");
	let cited = check().expect_code(4).json();
	assert_eq!(codes(&cited), ["unsupported-filter"], "{cited:#}");
	assert_eq!(cited["diagnostics"][0]["location"]["project"], "app");
	assert_eq!(cited["diagnostics"][0]["location"]["file"], "code.rs");

	repo.write(".gitattributes", "");
	repo.write(
		"app/.source-sentry/source-sentry.yaml",
		"version: 1\ntime-zone: Not/AZone\n",
	);
	let zone = check().expect_code(3).json();
	assert_eq!(codes(&zone), ["invalid-time-zone"], "{zone:#}");
	assert_eq!(zone["diagnostics"][0]["location"]["project"], "app");
}
