//! `track`, `track --from`, confirmation and paths.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo, document};

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
		run.stdout.contains("git add docs/source-sentry.lock"),
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
/// git, and absolute paths work too, including `C:\...` on Windows.
#[test]
fn paths_are_relative_to_the_current_directory() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");

	let run = repo.run_in("docs", &["attest", "guide.md", "--dry-run"]);
	run.expect_code(0);
	assert!(
		run.stdout.contains("Attest docs/guide.md"),
		"{}",
		run.stdout
	);
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
	repo.run(&["attest", absolute, "--dry-run"]).expect_code(0);
	repo
		.run_in(
			"src",
			&["--repo", "..", "attest", "../docs/guide.md", "--dry-run"],
		)
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
