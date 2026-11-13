//! The review journey: `track`, `attest`, a change, `check`, `diff`, and
//! `attest` with the expectations `diff` printed.

use chrono::{DateTime, SubsecRound as _, Utc};
use serde_json::json;

use crate::support::{AT, TestRepo, document};

/// Interactive review refuses redirected input or output and offers the
/// noninteractive review commands without changing anything.
#[test]
fn interactive_review_requires_terminal_input_and_output() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let before = repo.read("docs/source-sentry.lock");
	let run = repo.run(&["review"]);
	run.expect_code(1);
	assert!(run.stderr.contains("terminal input and output"));
	assert!(run.stderr.contains("diff"));
	assert!(run.stderr.contains("attest"));
	assert_eq!(repo.read("docs/source-sentry.lock"), before);
	repo.run_in("..", &["review"]).expect_code(1);
	let rejected = repo.run(&["review", "--yes"]);
	rejected.expect_code(64);
}

/// The `attest` command that `diff` ends with, as arguments.
fn printed_attest(stdout: &str) -> Vec<String> {
	let line = stdout.lines().last().expect("diff prints something").trim();
	let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
	assert_eq!(words[..2], ["source-sentry", "attest"], "last line: {line}");
	words[1..].to_vec()
}

#[test]
fn a_reviewed_change_is_attested_with_the_expectations_diff_printed() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	let check = repo.run(&["check", "--at", AT]);
	check.expect_code(1);
	assert!(check.stdout.contains("docs/guide.md"), "{}", check.stdout);

	let diff = repo.run(&["diff", "docs/guide.md"]);
	diff.expect_code(0);
	assert!(diff.stdout.contains("+fn b() {}"), "{}", diff.stdout);
	assert!(diff.stdout.contains(" fn a() {}"), "{}", diff.stdout);
	let attest = printed_attest(&diff.stdout);
	let oid = repo.git(&["hash-object", "src/a.rs"]);
	assert_eq!(
		attest,
		[
			"attest",
			"docs/guide.md",
			"--expect",
			&format!("src/a.rs@sha1:{oid}")
		]
	);

	let mut args: Vec<&str> = attest.iter().map(String::as_str).collect();
	args.push("--yes");
	let attested = repo.run(&args);
	attested.expect_code(0);
	assert!(
		attested.stdout.contains("Matches the expectations"),
		"{}",
		attested.stdout
	);
	repo.run(&["check", "--at", AT]).expect_code(0);
}

/// A change made after the review is refused, and nothing is
/// written.
#[test]
fn a_change_after_the_review_is_refused() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");
	let attest = printed_attest(&repo.run(&["diff", "docs/guide.md"]).stdout);

	repo.write("src/a.rs", "fn a() {}\nfn c() {}\n");
	let lockfile = repo.read("docs/source-sentry.lock");
	let mut args: Vec<&str> = attest.iter().map(String::as_str).collect();
	args.push("--yes");
	let refused = repo.run(&args);
	refused.expect_code(2);
	assert!(
		refused.stderr.contains("does not match the expectations"),
		"{}",
		refused.stderr
	);
	assert_eq!(repo.read("docs/source-sentry.lock"), lockfile);
}

/// A document without sources is reviewed by rereading it, so the command
/// that `diff` prints carries `--reread`, and it records the review.
#[test]
fn a_document_without_sources_is_attested_by_rereading_it() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--yes"])
		.expect_code(0);
	let diff = repo.run(&["diff", "docs/guide.md"]);
	diff.expect_code(0);
	assert!(
		diff.stdout.contains("declares no sources"),
		"{}",
		diff.stdout
	);
	let attest = printed_attest(&diff.stdout);
	assert_eq!(attest.last().map(String::as_str), Some("--reread"));
	let mut args: Vec<&str> = attest.iter().map(String::as_str).collect();
	args.push("--yes");
	repo.run(&args).expect_code(0);
	assert!(
		repo
			.read("docs/source-sentry.lock")
			.contains("\"reviewed\""),
		"{}",
		repo.read("docs/source-sentry.lock")
	);
}

/// A pattern that never matched a file and was never attested cannot be
/// attested, and `diff` says so beside it.
#[test]
fn diff_explains_a_pattern_that_attest_would_refuse() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/b.rs", "fn b() {}\n");
	repo.git(&["add", "src/b.rs"]);
	repo
		.run(&["track", "docs/guide.md", "--pattern", "/src/b*.rs", "--yes"])
		.expect_code(0);
	repo.git(&["rm", "--quiet", "--cached", "src/b.rs"]);
	let diff = repo.run(&["diff", "docs/guide.md"]);
	diff.expect_code(0);
	assert!(
		diff
			.stdout
			.contains("was never attested, so `attest` refuses it"),
		"{}",
		diff.stdout
	);
}

/// The JSON form of `diff`: its schema, the entry's states and unified
/// diff, the expectations and the `attest` command.
#[test]
fn diff_as_json() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");

	let run = repo.run(&["diff", "docs/guide.md", "--format", "json"]);
	run.expect_code(0);
	let diff = run.json();
	let oid = format!("sha1:{}", repo.git(&["hash-object", "src/a.rs"]));
	assert_eq!(diff["schema"], 1);
	assert_eq!(diff["document"], "docs/guide.md");
	assert_eq!(diff["expectations"], json!([format!("src/a.rs@{oid}")]));
	assert_eq!(
		diff["attest"],
		json!([
			"source-sentry",
			"attest",
			"docs/guide.md",
			"--expect",
			format!("/src/a.rs@{oid}")
		])
	);
	let entry = &diff["sources"][0]["entries"][0];
	assert_eq!(entry["changed"], true);
	assert_eq!(entry["baseline"]["state"], "available");
	assert_eq!(entry["current"], json!({"oid": oid, "state": "present"}));
	assert_eq!(entry["diff"], "@@ -1 +1,2 @@\n fn a() {}\n+fn b() {}\n");
}

/// Without expectations, `attest` records the working tree, and
/// says what it bound. `--source` limits the selection, and `--reviewer`
/// names who reviewed.
#[test]
fn attest_without_expectations_binds_the_current_state() {
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

	let before = Utc::now().trunc_subsecs(0);
	let run = repo.run(&[
		"attest",
		"docs/guide.md",
		"--source",
		"/src/a.rs",
		"--reviewer",
		"Legal Team",
		"--yes",
	]);
	let after = Utc::now();
	run.expect_code(0);
	assert!(
		run.stdout.contains("Binds the current state"),
		"{}",
		run.stdout
	);
	let lockfile: serde_json::Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	let sources = &lockfile["documents"]["guide.md"]["sources"];
	assert_eq!(sources[0]["reviewed"]["by"], "Legal Team");
	// The review is dated when `attest` runs, to the second.
	let at: DateTime<Utc> = sources[0]["reviewed"]["at"]
		.as_str()
		.expect("a review time")
		.parse()
		.expect("an RFC 3339 instant");
	assert!(
		before <= at && at <= after,
		"{at} is not between {before} and {after}"
	);
	assert_eq!(
		sources[1].get("reviewed"),
		None,
		"src/b.rs was not selected"
	);

	repo.commit_at("2026-10-04T10:00:00Z");
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let states: Vec<&str> = document(&report, "docs/guide.md")["sources"]
		.as_array()
		.unwrap()
		.iter()
		.map(|source| source["state"].as_str().unwrap())
		.collect();
	assert_eq!(states, ["fresh", "unattested"]);
}

/// A name remembered in git's configuration comes before `user.name`.
#[test]
fn a_remembered_reviewer_precedes_the_git_user_name() {
	let repo = TestRepo::init();
	repo.git(&["config", "source-sentry.reviewer", "Remembered Name"]);
	repo.reviewed_guide();
	let lockfile: serde_json::Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lockfile["documents"]["guide.md"]["sources"][0]["reviewed"]["by"],
		"Remembered Name"
	);
}

/// The reviewer defaults to git's `user.name`.
#[test]
fn the_reviewer_defaults_to_the_git_user_name() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let lockfile: serde_json::Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lockfile["documents"]["guide.md"]["sources"][0]["reviewed"]["by"],
		"Test Author"
	);
}

#[test]
fn a_malformed_declaration_is_reported_rather_than_untracked() {
	let repo = TestRepo::init();
	repo.write(
		"docs/guide.md",
		"---\nsource-sentry:\n  owners: [a]\n  owners: [b]\n---\nBody\n",
	);
	repo.commit_at("2026-11-01T12:00:00Z");

	for args in [
		&["attest", "docs/guide.md", "--yes"][..],
		&["diff", "docs/guide.md"],
	] {
		let command = args[0];
		let run = repo.run(args);
		assert_ne!(run.code, 0);
		assert!(
			run.stderr.contains("has errors to fix first"),
			"{command}: {}",
			run.stderr
		);
		assert!(
			!run.stderr.contains("Register it with `track`"),
			"{command}: {}",
			run.stderr
		);
	}
}
