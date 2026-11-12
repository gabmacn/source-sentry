//! `track`, `track --from`, confirmation and paths.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo, document};

/// `track` refuses both lockfile families with exit 2 and an actionable
/// explanation, even when the lockfile does not exist yet.
#[test]
fn track_refuses_lockfile_sources() {
	let repo = TestRepo::init();
	for source in [
		"/source-sentry.lock",
		"/docs/source-sentry.lock",
		"/.source-sentry/remotes/legal.lock",
	] {
		let run =
			repo.run(&["track", "docs/guide.md", "--source", source, "--yes"]);
		run.expect_code(2);
		assert!(
			run.stderr.contains("recording a review rewrites it"),
			"{}",
			run.stderr
		);
		assert!(!repo.exists("docs/source-sentry.lock"));
	}
}

/// Pattern reviews never include lockfiles, and lockfile-only patterns
/// are empty. Configuration files still participate in reviews.
#[test]
fn pattern_check_diff_and_attest_exclude_lockfiles() {
	let repo = TestRepo::init();
	repo.write(
		"inputs/source-sentry.lock",
		r#"{"documents": {}, "retired": {}, "schema": 1}"#,
	);
	repo.write("inputs/source-sentry.yaml", "{}\n");
	repo.write(
		".source-sentry/remotes/legal.lock",
		r#"{"documents": {}, "retired": {}, "schema": 1}"#,
	);
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--pattern",
			"/**/source-sentry.lock",
			"--yes",
		])
		.expect_code(2);
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--pattern",
			"/inputs/*",
			"--pattern",
			"/.source-sentry/remotes/*",
			"--yes",
		])
		.expect_code(2);
	repo
		.run(&["track", "docs/guide.md", "--pattern", "/inputs/*", "--yes"])
		.expect_code(0);
	let diff = repo.run(&["diff", "docs/guide.md", "--format", "json"]);
	diff.expect_code(0);
	assert!(
		!diff.stdout.contains("inputs/source-sentry.lock"),
		"{}",
		diff.stdout
	);
	assert!(diff.stdout.contains("inputs/source-sentry.yaml"));
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	let lock: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lock["documents"]["guide.md"]["sources"][0]["members"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	let check = repo.run(&["check", "--at", AT, "--format", "json"]);
	check.expect_code(0);
	assert!(!check.stdout.contains("inputs/source-sentry.lock"));
}

/// Existing declarations report a located configuration error instead of
/// attempting to review a lockfile.
#[test]
fn check_reports_lockfile_source_at_its_declaration() {
	let repo = TestRepo::init();
	repo.write("docs/guide.md", "---\nsource-sentry:\n  sources:\n    - path: /source-sentry.lock\n---\n# Guide\n");
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(3);
	let report = run.json();
	let diagnostic = report["diagnostics"]
		.as_array()
		.unwrap()
		.iter()
		.find(|diagnostic| diagnostic["code"] == "lockfile-source")
		.unwrap();
	assert_eq!(diagnostic["severity"], "configuration-error");
	assert_eq!(diagnostic["location"]["file"], "docs/guide.md");
	assert_eq!(diagnostic["location"]["line"], 4);
}

/// A submodule has no content to review, so `track` refuses to declare it,
/// as a path or as a new pattern's match, rather than leave every later
/// `check` to report it.
#[test]
fn track_refuses_a_populated_submodule_source() {
	let repo = TestRepo::init();
	repo.add_populated_submodule();
	let before = repo.read("docs/guide.md");
	for args in [["--source", "/vendor/lib"], ["--pattern", "/vendor/*"]] {
		let mut command = vec!["track", "docs/guide.md"];
		command.extend(args);
		command.push("--yes");
		let run = repo.run(&command);
		run.expect_code(2);
		assert!(
			run
				.stderr
				.contains("names `vendor/lib`, which is a submodule"),
			"{}",
			run.stderr
		);
		assert!(run.stderr.contains(args[1]), "{}", run.stderr);
		assert!(!repo.exists("docs/source-sentry.lock"));
		assert_eq!(repo.read("docs/guide.md"), before);
	}
}

#[test]
fn track_refuses_a_file_inside_a_populated_submodule() {
	let repo = TestRepo::init();
	repo.add_populated_submodule();
	let before = repo.read("docs/guide.md");
	let run = repo.run(&[
		"track",
		"docs/guide.md",
		"--source",
		"/vendor/lib/README.md",
		"--yes",
	]);
	run.expect_code(2);
	assert!(
		run.stderr.contains("vendor/lib/README.md"),
		"{}",
		run.stderr
	);
	assert!(
		run.stderr.contains("not tracked in this snapshot"),
		"{}",
		run.stderr
	);
	assert!(!repo.exists("docs/source-sentry.lock"));
	assert_eq!(repo.read("docs/guide.md"), before);
}

/// `track --from` moves an on-file declaration into a
/// lockfile, keeping its attestations.
#[test]
fn track_from_moves_a_declaration_between_placements() {
	let repo = TestRepo::init();
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--on-file",
			"--source",
			"/src/a.rs",
			"--yes",
		])
		.expect_code(0);
	assert!(repo.read("docs/guide.md").contains("source-sentry:"));
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	let before = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	assert_eq!(document(&before, "docs/guide.md")["mode"], "on-file");

	let run = repo.run(&[
		"track",
		"docs/guide.md",
		"--from",
		"on-file",
		"--to",
		"lockfile:docs",
		"--yes",
	]);
	run.expect_code(0);
	assert!(run.stdout.contains("Move docs/guide.md"), "{}", run.stdout);
	assert!(
		run.stdout.contains("git add -- docs/source-sentry.lock"),
		"{}",
		run.stdout
	);
	assert!(!repo.read("docs/guide.md").contains("source-sentry:"));
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert!(
		lockfile["documents"]["guide.md"]["sources"][0]["oid"].is_string(),
		"the attestation moves with the declaration: {lockfile:#}"
	);

	repo.git(&["add", "docs/source-sentry.lock"]);
	let after = repo.run(&["check", "--at", AT, "--format", "json"]);
	after.expect_code(0);
	let guide = document(&after.json(), "docs/guide.md").clone();
	assert_eq!(guide["mode"], "lockfile");
	assert_eq!(guide["facts"], json!(["fresh"]));
}

/// Without a terminal and without `--yes`, nothing changes.
#[test]
fn without_a_terminal_or_yes_nothing_changes() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	// A change to attest, so that the plan is never empty, even when this
	// attestation falls in the same second as the one above.
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	let lockfile = repo.read("docs/source-sentry.lock");
	let run = repo.run(&["attest", "docs/guide.md"]);
	run.expect_code(1);
	assert!(run.stderr.contains("--yes"), "{}", run.stderr);
	assert!(
		run.stdout.contains("Attest docs/guide.md"),
		"the plan is shown"
	);
	assert_eq!(repo.read("docs/source-sentry.lock"), lockfile);

	let track = repo.run(&["track", "src/a.rs"]);
	track.expect_code(1);
	assert!(!repo.exists("src/source-sentry.lock"));
}

/// `--dry-run` prints the plan and writes nothing, even with
/// `--yes`.
#[test]
fn dry_run_changes_nothing() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	let lockfile = repo.read("docs/source-sentry.lock");
	for args in [
		&["attest", "docs/guide.md", "--dry-run"][..],
		&["attest", "docs/guide.md", "--dry-run", "--yes"],
		&["seal", "docs/guide.md", "--dry-run"],
		&["retire", "docs/guide.md", "--dry-run"],
	] {
		let run = repo.run(args);
		run.expect_code(0);
		assert!(
			run.stdout.contains("Dry run: nothing was written."),
			"{args:?}"
		);
		assert_eq!(repo.read("docs/source-sentry.lock"), lockfile, "{args:?}");
	}
	repo.run(&["track", "src/a.rs", "--dry-run"]).expect_code(0);
	assert!(!repo.exists("src/source-sentry.lock"));
}

/// Document paths are relative to the current directory, as in
/// git. Native drive-qualified paths work on Windows.
#[test]
fn paths_are_relative_to_the_current_directory() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");

	let run = repo.run_in("docs", &["attest", "guide.md", "--dry-run"]);
	run.expect_code(0);
	assert!(run.stdout.contains("Attest guide.md"), "{}", run.stdout);
	let diff = repo.run_in("src", &["diff", "../docs/guide.md"]);
	diff.expect_code(0);
	assert!(
		diff
			.stdout
			.lines()
			.last()
			.unwrap()
			.contains("attest ../docs/guide.md"),
		"the printed command names the document from the same directory: {}",
		diff.stdout
	);

	let absolute = repo.path().join("docs").join("guide.md");
	let absolute = absolute.to_str().unwrap();
	if cfg!(windows) {
		repo.run(&["attest", absolute, "--dry-run"]).expect_code(0);
	}
	repo
		.run_in("src", &["-C", "..", "attest", "docs/guide.md", "--dry-run"])
		.expect_code(0);
	repo
		.run(&["attest", "docs/missing.md", "--yes"])
		.expect_code(2);
}

/// A remote document is tracked by identity in its
/// owner's lockfile, with sources rooted at the repository.
#[test]
fn remote_documents_are_tracked_by_identity() {
	let repo = TestRepo::init();
	let run = repo.run(&[
		"track",
		"abstract:intro-video",
		"--remote",
		"marketing",
		"--source",
		"/src/a.rs",
		"--url",
		"https://example.com/intro",
		"--yes",
	]);
	run.expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read(".source-sentry/remotes/marketing.lock"))
			.unwrap();
	assert_eq!(
		lockfile["documents"]["abstract:intro-video"]["url"],
		"https://example.com/intro"
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let video = document(&report, "abstract:intro-video");
	assert_eq!(video["mode"], "remote");
	assert_eq!(
		video["declared-in"],
		".source-sentry/remotes/marketing.lock"
	);
}

/// `track` writes classes, owners, a grace period
/// and a cadence, or `cadence: null`, and keeps the sources in the order
/// given.
#[test]
fn track_writes_the_declaration_it_is_given() {
	let repo = TestRepo::init();
	repo.write("src/b.rs", "fn b() {}\n");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--on-file",
			"--source",
			"/src/b.rs",
			"--pattern",
			"/src/*.rs",
			"--source",
			"/src/a.rs",
			"--owner",
			"platform",
			"--grace",
			"14",
			"--cadence",
			"2026-12-01",
			"FREQ=MONTHLY;INTERVAL=2",
			"--yes",
		])
		.expect_code(0);
	let guide = repo.read("docs/guide.md");
	let b = guide.find("'/src/b.rs'").expect("first source");
	let pattern = guide.find("'/src/*.rs'").expect("the pattern");
	let a = guide.find("'/src/a.rs'").expect("last source");
	assert!(b < pattern && pattern < a, "{guide}");
	assert!(guide.contains("grace: 14"), "{guide}");
	assert!(guide.contains("FREQ=MONTHLY;INTERVAL=2"), "{guide}");
	assert!(guide.contains("platform"), "{guide}");

	repo.write("docs/other.md", "# Other\n");
	repo
		.run(&[
			"track",
			"docs/other.md",
			"--on-file",
			"--no-cadence",
			"--yes",
		])
		.expect_code(0);
	assert!(repo.read("docs/other.md").contains("cadence: null"));
}

/// `track` on a tracked document adds to its declaration where it is,
/// keeping the review it had.
#[test]
fn track_extends_a_tracked_document() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/b.rs", "fn b() {}\n");
	repo.commit_at("2026-10-03T10:00:00Z");

	let run = repo.run(&[
		"track",
		"docs/guide.md",
		"--source",
		"/src/b.rs",
		"--owner",
		"docs",
		"--yes",
	]);
	run.expect_code(0);
	assert!(
		run
			.stdout
			.contains("Add to the declaration of docs/guide.md"),
		"{}",
		run.stdout
	);
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	let entry = &lockfile["documents"]["guide.md"];
	assert_eq!(entry["owners"], json!(["docs"]));
	let sources = entry["sources"].as_array().unwrap();
	assert_eq!(sources.len(), 2, "{entry}");
	assert!(
		sources[0]["oid"].is_string(),
		"the old review stays: {entry}"
	);
	assert!(
		sources[1].get("oid").is_none(),
		"the new source is unattested: {entry}"
	);

	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let facts = &document(&report, "docs/guide.md")["facts"];
	assert!(
		facts.as_array().unwrap().contains(&json!("stale")),
		"{facts}"
	);
}

/// Extending refuses a source already declared, another placement, and a
/// bare `track` of a tracked document.
#[test]
fn track_refuses_what_cannot_extend() {
	let repo = TestRepo::init();
	repo.reviewed_guide();

	let duplicate =
		repo.run(&["track", "docs/guide.md", "--source", "src/a.rs", "--yes"]);
	assert_ne!(duplicate.code, 0);
	assert!(
		duplicate.stderr.contains("src/a.rs"),
		"{}",
		duplicate.stderr
	);

	let elsewhere = repo.run(&[
		"track",
		"docs/guide.md",
		"--on-file",
		"--owner",
		"docs",
		"--yes",
	]);
	assert_ne!(elsewhere.code, 0);
	assert!(
		elsewhere.stderr.contains("track --from"),
		"{}",
		elsewhere.stderr
	);

	let bare = repo.run(&["track", "docs/guide.md", "--yes"]);
	assert_ne!(bare.code, 0);
	assert!(bare.stderr.contains("already tracked"), "{}", bare.stderr);
	assert!(!repo.read("docs/guide.md").contains("source-sentry"));
}

/// `untrack` removes a declared source, and the others keep their review.
#[test]
fn untrack_removes_a_source() {
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
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");

	let run =
		repo.run(&["untrack", "docs/guide.md", "--source", "src/b.rs", "--yes"]);
	run.expect_code(0);
	assert!(run.stdout.contains("1 source remain"), "{}", run.stdout);
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	let sources = lockfile["documents"]["guide.md"]["sources"]
		.as_array()
		.unwrap();
	assert_eq!(sources.len(), 1);
	assert_eq!(sources[0]["path"], "/src/a.rs");
	assert!(sources[0]["oid"].is_string());

	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	assert_eq!(
		document(&report, "docs/guide.md")["facts"],
		json!(["fresh"])
	);
}

/// `untrack` is how a missing source is fixed, and it refuses a source the
/// declaration does not have, or no source at all.
#[test]
fn untrack_fixes_a_missing_source_and_refuses_unknown_ones() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["rm", "--quiet", "src/a.rs"]);
	repo.commit_at("2026-10-03T10:00:00Z");
	repo.run(&["check", "--at", AT]).expect_code(3);

	let unknown =
		repo.run(&["untrack", "docs/guide.md", "--source", "/src/z.rs", "--yes"]);
	assert_ne!(unknown.code, 0);
	assert!(
		unknown.stderr.contains("declares no source"),
		"{}",
		unknown.stderr
	);
	repo
		.run(&["untrack", "docs/guide.md", "--yes"])
		.expect_code(64);

	repo
		.run(&["untrack", "docs/guide.md", "--source", "/src/a.rs", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-04T10:00:00Z");
	// No configuration error remains. With no source and no review left,
	// the document is unattested.
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(1);
	assert!(run.stdout.contains("unattested"), "{}", run.stdout);
	assert!(!run.stdout.contains("missing-source"), "{}", run.stdout);
}

/// A pattern that lost its matches since it was declared does not block an
/// unrelated extension; only a new pattern must match.
#[test]
fn an_old_empty_pattern_does_not_block_an_extension() {
	let repo = TestRepo::init();
	repo.write("src/a.txt", "a\n");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--pattern",
			"/src/a*.txt",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.git(&["rm", "--quiet", "src/a.txt"]);
	repo.commit_at("2026-10-02T10:00:00Z");

	repo
		.run(&["track", "docs/guide.md", "--owner", "legal", "--yes"])
		.expect_code(0);
	let empty = repo.run(&[
		"track",
		"docs/guide.md",
		"--pattern",
		"/src/b*.txt",
		"--yes",
	]);
	assert_ne!(empty.code, 0);
	assert!(
		empty.stderr.contains("matches no tracked file"),
		"{}",
		empty.stderr
	);
}

/// A blank owner is refused on file as it is in a directory config.
#[test]
fn a_blank_owner_is_refused_on_file() {
	let repo = TestRepo::init();
	repo.write(
		"docs/guide.md",
		"---\nsource-sentry:\n  owners: ['  ']\n---\n# Guide\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(3);
	assert!(run.stdout.contains("owners[0]"), "{}", run.stdout);
	assert!(run.stdout.contains("blank"), "{}", run.stdout);
}

/// With `-C`, the suggested `git add` names the repository, so that it
/// works from where the command was run.
#[test]
fn the_git_add_hint_follows_repo() {
	let repo = TestRepo::init();
	let run = repo.run_in(
		"src",
		&[
			"-C",
			"..",
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--yes",
		],
	);
	run.expect_code(0);
	assert!(
		run
			.stdout
			.contains("git -C .. add -- docs/source-sentry.lock"),
		"{}",
		run.stdout
	);
}
