//! `impact`, and the citations `explain` shows, through the executable and
//! real git snapshots.

use std::fs;

use serde_json::Value;

use crate::support::{AT, TestRepo, document};

/// `docs/guide.md` against the pattern `/src/*.rs`, `docs/overview.md`
/// citing it, and `docs/old.md` sealed against `src/a.rs`, all committed.
fn chain() -> TestRepo {
	let repo = TestRepo::init();
	repo.write("docs/overview.md", "# Overview\n");
	repo.write("docs/old.md", "# Old\n");
	for (document, option, source) in [
		("docs/guide.md", "--pattern", "/src/*.rs"),
		("docs/overview.md", "--source", "/docs/guide.md"),
		("docs/old.md", "--source", "/src/a.rs"),
	] {
		repo
			.run(&["track", document, option, source, "--yes"])
			.expect_code(0);
	}
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.run(&["attest", "docs/old.md", "--yes"]).expect_code(0);
	repo.run(&["seal", "docs/old.md", "--yes"]).expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	repo
}

/// The documents of a JSON answer, as `(project, document, depth)`.
fn documents(json: &Value) -> Vec<(String, String, u64)> {
	json["documents"]
		.as_array()
		.unwrap()
		.iter()
		.map(|document| {
			(
				document["project"].as_str().unwrap().to_owned(),
				document["document"].as_str().unwrap().to_owned(),
				document["depth"].as_u64().unwrap(),
			)
		})
		.collect()
}

fn entry(project: &str, document: &str, depth: u64) -> (String, String, u64) {
	(project.to_owned(), document.to_owned(), depth)
}

/// One list: direct matches, then the documents citing them, then sealed
/// records; and the files nothing cites.
#[test]
fn the_answer_lists_direct_then_cited_documents_then_records() {
	let repo = chain();
	let run = repo.run(&["impact", "src/a.rs", "src/none.txt"]);
	run.expect_code(0);
	let text = &run.stdout;
	let direct = text.find("direct  docs/guide.md").expect(text);
	let via = text.find("via     docs/overview.md").expect(text);
	let sealed = text.find("sealed  docs/old.md").expect(text);
	assert!(direct < via && via < sealed, "{text}");
	assert!(text.contains("src/a.rs, declared as /src/*.rs"), "{text}");
	assert!(
		text.contains("cites docs/guide.md, 2 steps from a file"),
		"{text}"
	);
	assert!(
		text.contains(
			"a sealed record never changes, so these files do not affect it"
		),
		"{text}"
	);
	assert!(text.contains("No document cites `src/none.txt`."), "{text}");
	assert!(
		text.ends_with(
			"1 direct · 1 through other documents · 1 sealed record · exit 0\n"
		),
		"{text}"
	);

	let json = repo
		.run(&["impact", "src/a.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(
		documents(&json),
		[
			entry(".", "docs/guide.md", 1),
			entry(".", "docs/overview.md", 2),
			entry(".", "docs/old.md", 1),
		]
	);
	let guide = &json["documents"][0];
	assert_eq!(guide["reached"]["kind"], "direct");
	assert_eq!(guide["reached"]["matches"][0]["path"], "src/a.rs");
	assert_eq!(guide["reached"]["matches"][0]["source"], "/src/*.rs");
	assert_eq!(guide["sealed"], false);
	assert_eq!(guide["mode"], "lockfile");
	assert_eq!(guide["owner"], Value::Null);
	assert_eq!(json["documents"][1]["reached"]["kind"], "via");
	assert_eq!(json["documents"][1]["reached"]["document"], "docs/guide.md");
	assert_eq!(json["documents"][2]["sealed"], true);
	assert_eq!(json["inputs"][0]["matched"], true);
	assert_eq!(json["schema"], 1);
	assert_eq!(json["revision"]["kind"], "working-tree");
	assert_eq!(json["not-searched"], Value::Array(Vec::new()));

	// The settings are those `check` reports for the same document.
	let check = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	assert_eq!(
		guide["settings"],
		document(&check, "docs/guide.md")["settings"]
	);

	let quiet = repo.run(&["impact", "src/none.txt"]);
	quiet.expect_code(0);
	assert!(
		quiet
			.stdout
			.contains("No document in the searched project cites this file."),
		"{}",
		quiet.stdout
	);
}

/// A file a pattern would take and a deleted source both count, and a
/// deleted source, which `check` refuses as a configuration error, does not
/// stop the query.
#[test]
fn new_and_deleted_files_count() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--source", "/src/a.rs", "--yes"])
		.expect_code(0);
	repo.write("docs/api.md", "# API\n");
	repo
		.run(&["track", "docs/api.md", "--pattern", "/src/**", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.git(&["rm", "--quiet", "src/a.rs"]);
	assert_eq!(repo.run(&["check", "--at", AT]).code, 3);
	let json = repo
		.run(&["impact", "src/a.rs", "src/new/b.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(
		documents(&json),
		[entry(".", "docs/api.md", 1), entry(".", "docs/guide.md", 1)]
	);
	assert_eq!(json["diagnostics"], Value::Array(Vec::new()));
}

/// `--changed` takes `git status`'s files, untracked ones included one by
/// one; `--staged` only the index's; `-` reads standard input, by line or
/// by NUL.
#[test]
fn inputs_come_from_git_and_from_standard_input() {
	let repo = chain();
	// Written without staging, unlike `TestRepo::write`.
	let unstaged = |file: &str, contents: &str| {
		let full = repo.path().join(file);
		fs::create_dir_all(full.parent().unwrap()).unwrap();
		fs::write(full, contents).unwrap();
	};
	unstaged("src/a.rs", "fn a() { changed() }\n");
	unstaged("src/new/deep.rs", "fn deep() {}\n");
	unstaged("src/fresh.rs", "fn fresh() {}\n");
	let inputs = |run: &crate::support::Run| -> Vec<String> {
		run.json()["inputs"]
			.as_array()
			.unwrap()
			.iter()
			.map(|input| input["path"].as_str().unwrap().to_owned())
			.collect()
	};
	let changed = repo.run(&["impact", "--changed", "--format", "json"]);
	changed.expect_code(0);
	assert_eq!(
		inputs(&changed),
		["src/a.rs", "src/fresh.rs", "src/new/deep.rs"]
	);
	let nothing = repo.run(&["impact", "--staged", "--format", "json"]);
	assert_eq!(inputs(&nothing), Vec::<String>::new());
	repo.git(&["add", "src/fresh.rs"]);
	let staged = repo.run(&["impact", "--staged", "--format", "json"]);
	assert_eq!(inputs(&staged), ["src/fresh.rs"]);
	assert_eq!(documents(&staged.json())[0], entry(".", "docs/guide.md", 1));
	// A clean query is still an answer.
	repo.commit_at("2026-10-03T10:00:00Z");
	repo.git(&["add", "--all"]);
	repo.commit_at("2026-10-03T11:00:00Z");
	repo.run(&["impact", "--changed"]).expect_code(0);

	// Standard input: repository-relative, from any directory.
	let lines = repo.run_with_input(
		&["-C", "docs", "impact", "-", "--format", "json"],
		b"src/a.rs\r\n\r\nsrc/none.txt\n",
	);
	lines.expect_code(0);
	// Relative to the project, which `docs/` is not: not `docs/src/a.rs`.
	assert_eq!(inputs(&lines), ["src/a.rs", "src/none.txt"]);
	let nul = repo
		.run_with_input(&["impact", "-", "-z", "--format", "json"], b"src/a.rs\0");
	assert_eq!(inputs(&nul), ["src/a.rs"]);
	repo
		.run_with_input(&["impact", "-"], b"\"src/\\303\\251.rs\"\n")
		.expect_code(64);
	repo.run_with_input(&["impact", "-"], b"").expect_code(0);
	// Unmatched files from standard input are counted, not listed.
	let counted =
		repo.run_with_input(&["impact", "-"], b"src/a.rs\nsrc/none.txt\n");
	assert!(
		counted
			.stdout
			.contains("1 other file is cited by no document."),
		"{}",
		counted.stdout
	);
}

/// What `impact` refuses as usage errors.
#[test]
fn usage_errors() {
	let repo = chain();
	for args in [
		&["impact"][..],
		&["impact", "src"],
		&["impact", "."],
		&["impact", "src/a.rs", "-z"],
		&["impact", "-", "-"],
		&["impact", "--rev", "HEAD", "--changed"],
		&["impact", "--rev", "HEAD", "--staged"],
	] {
		repo.run(args).expect_code(64);
	}
	let run = repo.run(&["impact", "src"]);
	assert!(
		run.stderr.contains("`src` is a directory"),
		"{}",
		run.stderr
	);
	// A committed snapshot's declarations.
	let json = repo
		.run(&["impact", "--rev", "HEAD~1", "src/a.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(json["revision"]["kind"], "commit");
	assert_eq!(documents(&json).len(), 3);
}

/// A nested project with a local document, and documents of the enclosing
/// project that cite its file.
fn nested() -> TestRepo {
	let repo = TestRepo::init();
	repo.write(
		"app/.source-sentry/source-sentry.yaml",
		"version: 1\nschedule: {grace: 5}\n",
	);
	repo.write("app/guide.md", "# Nested guide\n");
	repo.write("app/code.rs", "fn nested() {}\n");
	repo
		.run(&["track", "app/guide.md", "--source", "/code.rs", "--yes"])
		.expect_code(0);
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/app/code.rs",
			"--yes",
		])
		.expect_code(0);
	repo
		.run(&[
			"track",
			"abstract:video",
			"--remote",
			"team",
			"--source",
			"/app/code.rs",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
}

/// Each document is written relative to its own project; the projects
/// outside the starting one are not searched, and the output says so.
#[test]
fn nested_projects_are_searched_from_where_the_query_runs() {
	let repo = nested();
	let json = repo
		.run(&["impact", "app/code.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(
		documents(&json),
		[
			entry(".", "docs/guide.md", 1),
			entry(".", "abstract:video", 1),
			entry("app", "guide.md", 1),
		]
	);
	let nested = &json["documents"][2];
	assert_eq!(nested["reached"]["matches"][0]["path"], "code.rs");
	assert_eq!(nested["reached"]["matches"][0]["source"], "/code.rs");
	assert_eq!(nested["settings"]["grace"]["value"], 5);
	assert_eq!(json["documents"][1]["mode"], "remote");
	assert_eq!(json["documents"][1]["owner"], "team");
	assert_eq!(json["project"], ".");
	assert_eq!(json["inputs"][0]["path"], "app/code.rs");

	let inside = repo
		.run(&["-C", "app", "impact", "code.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(documents(&inside), [entry("app", "guide.md", 1)]);
	assert_eq!(inside["project"], "app");
	assert_eq!(inside["inputs"][0]["path"], "code.rs");
	assert_eq!(inside["inputs"][0]["searched"], true);
	assert_eq!(inside["not-searched"], Value::Array(vec![".".into()]));
	let text = repo.run(&["-C", "app", "impact", "code.rs", "../src/a.rs"]);
	assert!(
		text.stdout.contains(
			"note  Project not searched: `..`. A document there may still cite"
		),
		"{}",
		text.stdout
	);
	assert!(
		text.stdout.contains(
			"note  `../src/a.rs` lies in project `..`, which was not searched."
		),
		"{}",
		text.stdout
	);
}

/// An unreadable lockfile hides its documents: the query answers from what
/// it could read, lists the error, and exits 3.
#[test]
fn errors_that_may_hide_documents_exit_3_after_answering() {
	let repo = nested();
	repo.write("app/source-sentry.lock", "{ not json");
	let run = repo.run(&["impact", "app/code.rs"]);
	run.expect_code(3);
	assert!(run.stdout.contains("docs/guide.md"), "{}", run.stdout);
	assert!(run.stdout.contains("invalid-lockfile"), "{}", run.stdout);
	assert!(
		run.stdout.contains("These errors may hide documents"),
		"{}",
		run.stdout
	);
	let json = repo
		.run(&["impact", "app/code.rs", "--format", "json"])
		.expect_code(3)
		.json();
	assert_eq!(json["diagnostics"][0]["code"], "invalid-lockfile");
	assert_eq!(json["diagnostics"][0]["location"]["project"], "app");
	assert_eq!(documents(&json).len(), 2);
}

/// `explain` on a source file lists the documents citing it directly,
/// searched from where it runs; on a document, a "Cited by" section.
#[test]
fn explain_lists_the_documents_citing_a_file() {
	let repo = nested();
	let run = repo.run(&["explain", "app/code.rs", "--at", AT]);
	run.expect_code(0);
	let text = &run.stdout;
	assert!(
		text.contains("File  a source, cited directly by 3 documents"),
		"{text}"
	);
	assert!(text.contains("Cited by"), "{text}");
	for name in ["abstract:video", "docs/guide.md", "app/guide.md"] {
		assert!(text.contains(name), "{name}: {text}");
	}
	assert!(
		text.contains("`source-sentry impact app/code.rs` also lists"),
		"{text}"
	);

	// From inside the nested project, the enclosing one is not searched.
	let inside = repo.run(&["-C", "app", "explain", "code.rs", "--at", AT]);
	inside.expect_code(0);
	assert!(
		inside.stdout.contains("cited directly by 1 document"),
		"{}",
		inside.stdout
	);
	assert!(
		inside.stdout.contains("note  Project not searched: `..`"),
		"{}",
		inside.stdout
	);

	repo
		.run(&["explain", "src/a.rs", "--at", AT])
		.expect_code(2);

	let chain = chain();
	let guide = chain.run(&["explain", "docs/guide.md", "--at", AT]);
	guide.expect_code(0);
	let cited = guide.stdout.split("Cited by").nth(1).expect(&guide.stdout);
	assert!(cited.contains("docs/overview.md"), "{}", guide.stdout);
	assert!(
		cited.contains("declared as /docs/guide.md"),
		"{}",
		guide.stdout
	);
	let overview = chain.run(&["explain", "docs/overview.md", "--at", AT]);
	assert!(
		overview
			.stdout
			.contains("no document in the searched projects cites it"),
		"{}",
		overview.stdout
	);
}

/// In a `pre-commit` hook of `git commit -a`, `--staged` reads the commit's
/// temporary index, as every other command does there.
#[test]
fn staged_files_in_a_pre_commit_hook_are_the_commits() {
	let repo = chain();
	let report = repo.outer().join("impact.json");
	let hook = repo.path().join(".git/hooks/pre-commit");
	fs::create_dir_all(hook.parent().unwrap()).unwrap();
	let binary = crate::support::BINARY.replace('\\', "/");
	fs::write(
		&hook,
		format!(
			"#!/bin/sh\n\"{binary}\" impact --staged --format json > \"{}\"\n",
			report.to_str().unwrap().replace('\\', "/")
		),
	)
	.unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
	}
	repo.write("src/a.rs", "fn a() { changed() }\n");
	repo
		.git_at(
			repo.path(),
			&["commit", "--quiet", "-a", "-m", "change"],
			&[],
		)
		.expect_code(0);
	let json: Value =
		serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
	assert_eq!(json["inputs"][0]["path"], "src/a.rs", "{json}");
	assert_eq!(json["documents"][0]["document"], "docs/guide.md", "{json}");
}

/// A remote document of the nested project is written relative to it, and a
/// document of the enclosing project reached through a nested document names
/// that document relative to its own project.
#[test]
fn citations_cross_projects_in_both_directions() {
	let repo = nested();
	repo
		.run(&[
			"-C",
			"app",
			"track",
			"abstract:clip",
			"--remote",
			"crew",
			"--source",
			"/code.rs",
			"--yes",
		])
		.expect_code(0);
	repo.write("docs/overview.md", "# Overview\n");
	repo
		.run(&[
			"track",
			"docs/overview.md",
			"--source",
			"/app/guide.md",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	let json = repo
		.run(&["impact", "app/code.rs", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(
		documents(&json),
		[
			entry(".", "docs/guide.md", 1),
			entry(".", "abstract:video", 1),
			entry("app", "guide.md", 1),
			entry("app", "abstract:clip", 1),
			entry(".", "docs/overview.md", 2),
		]
	);
	let clip = &json["documents"][3];
	assert_eq!(clip["owner"], "crew");
	assert_eq!(clip["declared-in"], ".source-sentry/remotes/crew.lock");
	assert_eq!(clip["reached"]["matches"][0]["path"], "code.rs");
	let overview = &json["documents"][4];
	assert_eq!(overview["reached"]["kind"], "via");
	assert_eq!(overview["reached"]["document"], "app/guide.md");

	// `explain --rev` reads the commit's declarations.
	let before =
		repo.run(&["explain", "app/guide.md", "--rev", "HEAD~1", "--at", AT]);
	before.expect_code(0);
	let cited = before
		.stdout
		.split("Cited by")
		.nth(1)
		.expect(&before.stdout);
	assert!(
		cited.contains("no document in the searched projects"),
		"{}",
		before.stdout
	);
	let after = repo.run(&["explain", "app/guide.md", "--at", AT]);
	let cited = after.stdout.split("Cited by").nth(1).expect(&after.stdout);
	assert!(cited.contains("docs/overview.md"), "{}", after.stdout);
}

/// A name that nothing tracks or cites exits 2, even when an unreadable
/// lockfile may hide a document citing it, which the message names.
#[test]
fn an_uncited_name_exits_2_and_names_what_may_hide_a_citation() {
	let repo = nested();
	repo.write("app/source-sentry.lock", "{ not json");
	let run = repo.run(&["explain", "src/a.rs", "--at", AT]);
	run.expect_code(2);
	assert!(
		run.stderr.contains("no document cites it"),
		"{}",
		run.stderr
	);
	assert!(run.stderr.contains("invalid-lockfile"), "{}", run.stderr);
}

/// Before the first commit, every file in the index is staged, and
/// untracked files count as changed.
#[test]
fn changed_and_staged_files_before_the_first_commit() {
	let repo = TestRepo::init();
	fs::write(repo.path().join("untracked.txt"), "new\n").unwrap();
	let changed = repo
		.run(&["impact", "--changed", "--format", "json"])
		.expect_code(0)
		.json();
	let paths = |json: &Value| -> Vec<String> {
		json["inputs"]
			.as_array()
			.unwrap()
			.iter()
			.map(|input| input["path"].as_str().unwrap().to_owned())
			.collect()
	};
	assert_eq!(
		paths(&changed),
		["docs/guide.md", "src/a.rs", "untracked.txt"]
	);
	let staged = repo
		.run(&["impact", "--staged", "--format", "json"])
		.expect_code(0)
		.json();
	assert_eq!(paths(&staged), ["docs/guide.md", "src/a.rs"]);
}
