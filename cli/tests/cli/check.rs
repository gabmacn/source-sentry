//! `check`: exit codes under both thresholds, contexts, `--at`, `--rev` and
//! the JSON report.

use serde_json::json;

use crate::support::{AT, TestRepo, document, keys};

/// A repository config with a `privacy` class and a `production` context
/// that blocks privacy documents.
const PRODUCTION: &str = "\
version: 1
define:
  classes:
    privacy: {}
  contexts:
    production:
      fallback: warn
      rules:
        - facts: ['missed']
          outcome: notice
        - classes: ['privacy']
          facts: ['stale', 'overdue', 'unattested', 'timing-unknown', 'upstream-stale']
          outcome: fail
";

/// The exit codes of the report gate and the continue gate.
fn gates(repo: &TestRepo, extra: &[&str]) -> (i32, i32) {
	let run = |threshold: &str| {
		let mut args = vec!["check", "--at", AT, "--fail-on", threshold];
		args.extend_from_slice(extra);
		repo.run(&args).code
	};
	(run("warn"), run("fail"))
}

#[test]
fn a_fresh_document_passes_both_gates() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	assert_eq!(gates(&repo, &[]), (0, 0));
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(0);
	assert!(run.stdout.contains("1 passing · exit 0"), "{}", run.stdout);
}

#[test]
fn a_stale_document_warns_and_passes_the_continue_gate() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	assert_eq!(gates(&repo, &[]), (1, 0));
	let run = repo.run(&["check", "--at", AT]);
	assert!(run.stdout.contains("warn  docs/guide.md"), "{}", run.stdout);
	assert!(run.stdout.contains("stale"), "{}", run.stdout);
	assert!(
		run.stdout.contains("src/a.rs changed in"),
		"the changed source is named: {}",
		run.stdout
	);
}

#[test]
fn an_unattested_document_warns() {
	let repo = TestRepo::init();
	repo
		.run(&["track", "docs/guide.md", "--source", "/src/a.rs", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");

	assert_eq!(gates(&repo, &[]), (1, 0));
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let guide = document(&report, "docs/guide.md");
	assert!(
		guide["facts"]
			.as_array()
			.unwrap()
			.contains(&json!("unattested"))
	);
	assert_eq!(
		guide["unattested"]["since"], "2026-10-01T10:00:00Z",
		"the declaration's clock starts at the commit that added it"
	);
}

/// A stale document becomes overdue once its grace
/// period has passed at the instant given by `--at`.
#[test]
fn overdue_is_measured_at_the_given_instant() {
	let repo = TestRepo::init();
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--grace",
			"7",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
		.run(&["attest", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	let facts = |at: &str| {
		let report = repo.run(&["check", "--at", at, "--format", "json"]).json();
		let guide = document(&report, "docs/guide.md").clone();
		assert_eq!(guide["deadline"]["due"], "2026-10-27");
		assert_eq!(guide["deadline"]["overdue-from"], "2026-10-28T00:00:00Z");
		guide["facts"].clone()
	};
	assert_eq!(facts("2026-10-25T12:00:00Z"), json!(["stale"]));
	assert_eq!(facts("2026-10-28T00:00:00Z"), json!(["stale", "overdue"]));
	assert_eq!(gates(&repo, &[]), (1, 0));
}

/// A stale privacy policy fails production, while the default
/// context only warns.
#[test]
fn a_stale_privacy_policy_fails_production() {
	let repo = TestRepo::init();
	repo.write(".source-sentry/source-sentry.yaml", PRODUCTION);
	repo.write("legal/privacy.md", "# Privacy\n");
	repo.write("db/schema.sql", "create table users (id int);\n");
	repo
		.run(&[
			"track",
			"legal/privacy.md",
			"--source",
			"/db/schema.sql",
			"--class",
			"privacy",
			"--owner",
			"legal",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
		.run(&["attest", "legal/privacy.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	assert_eq!(gates(&repo, &["--context", "production"]), (0, 0));

	repo.write(
		"db/schema.sql",
		"create table users (id int, email text);\n",
	);
	repo.commit_at("2026-10-20T10:00:00Z");
	assert_eq!(gates(&repo, &["--context", "production"]), (2, 2));
	assert_eq!(gates(&repo, &[]), (1, 0));

	let run = repo.run(&["check", "--at", AT, "--context", "production"]);
	assert!(
		run.stdout.contains("fail  legal/privacy.md"),
		"{}",
		run.stdout
	);
	assert!(
		run
			.stdout
			.contains("production: privacy + stale, overdue → fail"),
		"the deciding rule is named: {}",
		run.stdout
	);
}

/// An unknown context is a configuration error, never the
/// default.
#[test]
fn an_unknown_context_is_a_configuration_error() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let run = repo.run(&["check", "--at", AT, "--context", "prodution"]);
	run.expect_code(3);
	assert!(run.stdout.contains("unknown-context"), "{}", run.stdout);
}

/// The JSON report parses, has the example's keys, and
/// carries both gates and the exit code.
#[test]
fn the_json_report_has_the_documented_keys() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(1);
	let report = run.json();
	assert_eq!(
		keys(&report),
		[
			"at",
			"context",
			"diagnostics",
			"exit-code",
			"gates",
			"revision",
			"schema",
			"subjects",
			"summary",
			"threshold",
		]
	);
	assert_eq!(report["schema"], 1);
	assert_eq!(report["at"], AT);
	assert_eq!(report["exit-code"], 1);
	assert_eq!(report["threshold"], "warn");
	assert_eq!(
		report["gates"],
		json!({"continue": "pass", "report": "fail"})
	);
	assert_eq!(report["context"]["name"], "default");
	assert_eq!(report["context"]["origin"], json!({"kind": "built-in"}));
	assert_eq!(report["revision"]["kind"], "working-tree");

	let guide = document(&report, "docs/guide.md");
	assert_eq!(
		keys(guide),
		[
			"deadline",
			"declared-in",
			"document",
			"facts",
			"field",
			"judgement",
			"kind",
			"mode",
			"outcome",
			"settings",
			"sources",
		]
	);
	assert_eq!(guide["kind"], "document");
	assert_eq!(guide["mode"], "lockfile");
	assert_eq!(guide["declared-in"], "docs/source-sentry.lock");
	assert_eq!(guide["sources"][0]["path"], "src/a.rs");
	assert_eq!(guide["sources"][0]["state"], "drifted");
	assert_eq!(
		guide["sources"][0]["clock"]["since"],
		"2026-10-20T10:00:00Z"
	);
	assert_eq!(guide["judgement"][0]["rule"], serde_json::Value::Null);

	let fail =
		repo.run(&["check", "--at", AT, "--format", "json", "--fail-on", "fail"]);
	fail.expect_code(0);
	assert_eq!(fail.json()["threshold"], "fail");
}

/// `--rev` evaluates a committed snapshot, not the working tree.
#[test]
fn rev_checks_a_committed_snapshot() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	assert_eq!(repo.run(&["check", "--at", AT]).code, 1);
	let run =
		repo.run(&["check", "--at", AT, "--rev", "HEAD~1", "--format", "json"]);
	run.expect_code(0);
	assert_eq!(run.json()["revision"]["kind"], "commit");
	assert_eq!(
		repo.run(&["check", "--at", AT, "--no-cache"]).code,
		1,
		"--no-cache is accepted and changes nothing"
	);
}

#[test]
fn quiet_prints_errors_only() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.write("src/a.rs", "fn a() {}\nfn b() {}\n");
	repo.commit_at("2026-10-20T10:00:00Z");

	let run = repo.run(&["--quiet", "check", "--at", AT]);
	run.expect_code(1);
	assert_eq!(run.stdout, "");
	let run = repo.run(&["--quiet", "check", "--at", AT, "--context", "nope"]);
	run.expect_code(3);
	assert!(run.stderr.contains("unknown-context"), "{}", run.stderr);
}
