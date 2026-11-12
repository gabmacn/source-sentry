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
fn a_missing_source_is_shown_under_its_passing_document() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["rm", "--quiet", "src/a.rs"]);
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(3);
	let lines: Vec<&str> = run.stdout.lines().collect();
	let page = lines
		.iter()
		.position(|line| line.starts_with("pass") && line.contains("docs/guide.md"))
		.unwrap_or_else(|| panic!("the passing page is listed:\n{}", run.stdout));
	assert!(
		lines[page + 1].contains("missing-source"),
		"the error follows the page:\n{}",
		run.stdout
	);
	assert_eq!(
		run.stdout.matches("missing-source").count(),
		1,
		"{}",
		run.stdout
	);
}

#[test]
fn files_outside_the_index_are_named_without_changing_the_exit() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	// Written without `git add`, unlike `TestRepo::write`.
	for (path, contents) in [
		("docs/new.md", "# New\n"),
		(".source-sentry/presets/team.yaml", "classes: []\n"),
		("notes.txt", "not in a root\n"),
	] {
		let full = repo.path().join(path);
		std::fs::create_dir_all(full.parent().unwrap()).unwrap();
		std::fs::write(full, contents).unwrap();
	}

	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(0);
	assert!(
		run
			.stdout
			.contains("2 files would count but are outside git's index"),
		"{}",
		run.stdout
	);
	assert!(
		run.stdout.contains("docs/new.md  a document"),
		"{}",
		run.stdout
	);
	assert!(
		run
			.stdout
			.contains(".source-sentry/presets/team.yaml  Source Sentry's own file"),
		"{}",
		run.stdout
	);
	assert!(!run.stdout.contains("notes.txt"), "{}", run.stdout);

	let committed = repo.run(&["check", "--at", AT, "--rev", "HEAD"]);
	committed.expect_code(0);
	assert!(
		!committed.stdout.contains("outside git's index"),
		"{}",
		committed.stdout
	);
}

/// Checks a hand-written declaration against both the working tree and the
/// committed tree, without relying on `track` to accept the source.
fn assert_populated_submodule_source(source: &str, code: &str, message: &str) {
	let repo = TestRepo::init();
	repo.add_populated_submodule();
	repo.write(
		"docs/guide.md",
		&format!(
			"---\nsource-sentry:\n  sources:\n    - path: /{source}\n---\n# Guide\n"
		),
	);
	repo.commit_at("2026-10-03T10:00:00Z");

	for revision in [&[][..], &["--rev", "HEAD"][..]] {
		let mut args = vec!["check", "--at", AT, "--format", "json"];
		args.extend_from_slice(revision);
		let run = repo.run(&args);
		run.expect_code(3);
		let report = run.json();
		assert_eq!(report["exit-code"], 3, "{report:#}");
		let diagnostics = report["diagnostics"].as_array().unwrap();
		assert_eq!(diagnostics.len(), 1, "{report:#}");
		let diagnostic = &diagnostics[0];
		assert_eq!(diagnostic["code"], code, "{report:#}");
		assert_eq!(diagnostic["severity"], "configuration-error");
		assert_eq!(diagnostic["location"]["file"], "docs/guide.md");
		assert_eq!(
			diagnostic["location"]["field"],
			"source-sentry.sources[0].path"
		);
		assert!(
			diagnostic["message"].as_str().unwrap().contains(message),
			"{diagnostic:#}"
		);
		assert!(
			diagnostic["message"].as_str().unwrap().contains(source),
			"{diagnostic:#}"
		);
	}

	let human = repo.run(&["check", "--at", AT]);
	human.expect_code(3);
	assert!(human.stdout.contains(code), "{}", human.stdout);
	assert!(human.stdout.contains(source), "{}", human.stdout);
}

#[test]
fn a_populated_submodule_source_is_a_configuration_error() {
	assert_populated_submodule_source(
		"vendor/lib",
		"submodule",
		"is a submodule, which Source Sentry does not support",
	);
}

#[test]
fn a_file_inside_a_populated_submodule_is_a_missing_source() {
	assert_populated_submodule_source(
		"vendor/lib/README.md",
		"missing-source",
		"is not tracked in this snapshot",
	);
}

#[test]
fn invalid_configs_still_give_a_json_report() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\nclasses: [missing]\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");

	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(3);
	let report = run.json();
	assert_eq!(report["exit-code"], 3, "{report}");
	assert_eq!(report["subjects"], json!([]), "{report}");
	assert!(report["context"].is_null(), "{report}");
	let diagnostics = report["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 1, "{report}");
	assert_eq!(diagnostics[0]["code"], "invalid-config");
	assert_eq!(diagnostics[0]["severity"], "configuration-error");
	assert_eq!(
		diagnostics[0]["location"]["file"],
		".source-sentry/source-sentry.yaml"
	);

	let human = repo.run(&["check", "--at", AT]);
	human.expect_code(3);
	assert!(
		human.stderr.contains("the configs are invalid"),
		"{}",
		human.stderr
	);
}

/// Rule diagnostics locate the named field in both report formats.
fn assert_rule_field_locations(config: &str, expected: &[(&str, u64)]) {
	let repo = TestRepo::init();
	let file = ".source-sentry/source-sentry.yaml";
	repo.write(file, config);
	repo.commit_at("2026-10-01T10:00:00Z");
	for revision in [&[][..], &["--rev", "HEAD"][..]] {
		let mut args = vec!["check", "--at", AT, "--format", "json"];
		args.extend_from_slice(revision);
		let run = repo.run(&args);
		run.expect_code(3);
		let report = run.json();
		let diagnostics = report["diagnostics"].as_array().unwrap();
		assert_eq!(diagnostics.len(), expected.len(), "{report:#}");
		for (field, line) in expected {
			let diagnostic = diagnostics
				.iter()
				.find(|diagnostic| diagnostic["location"]["field"] == *field)
				.unwrap_or_else(|| panic!("no diagnostic for {field}: {report:#}"));
			assert_eq!(diagnostic["location"]["file"], file);
			assert_eq!(diagnostic["location"]["line"], *line, "{diagnostic:#}");
		}
	}
	let run = repo.run(&["check", "--at", AT]);
	run.expect_code(3);
	for (_, line) in expected {
		assert!(
			run.stderr.contains(&format!("{file}:{line}:")),
			"{}",
			run.stderr
		);
	}
}

/// Scoped rules retain each classes field, including later rules and flow YAML.
#[test]
fn scoped_rule_class_errors_point_at_their_classes_fields() {
	assert_rule_field_locations(
		"version: 1\ndefine: {}\nrules:\n  default:\n    - facts: [stale]\n      \
		 outcome: fail\n      classes:\n        - missing\n    - facts: [stale]\n      \
		 outcome: warn\n      classes: [other]\n    - {facts: [stale], classes: [last], outcome: warn}\n",
		&[
			("rules.default[0].classes", 7),
			("rules.default[1].classes", 11),
			("rules.default[2].classes", 12),
		],
	);
}

/// Context definitions retain the classes fields independently of rule starts.
#[test]
fn context_rule_class_errors_point_at_their_classes_fields() {
	assert_rule_field_locations(
		"version: 1\ndefine:\n  contexts:\n    production:\n      fallback: pass\n      \
		 rules:\n        - facts: [stale]\n          outcome: fail\n          classes:\n            \
		 - missing\n        - facts: [stale]\n          outcome: warn\n          classes: [other]\n",
		&[
			("define.contexts.production.rules[0].classes", 9),
			("define.contexts.production.rules[1].classes", 13),
		],
	);
}

/// Reordering a scoped rule does not move a facts error onto its classes.
#[test]
fn scoped_rule_fact_errors_point_at_their_facts_fields() {
	assert_rule_field_locations(
		"version: 1\nrules:\n  default:\n    - classes: []\n      outcome: fail\n      \
		 facts: [fresh]\n    - classes: []\n      outcome: fail\n      facts: []\n",
		&[("rules.default[0].facts", 6), ("rules.default[1].facts", 9)],
	);
}

/// A context rule's fresh-fact error also names its own facts field.
#[test]
fn context_rule_fact_errors_point_at_their_facts_fields() {
	assert_rule_field_locations(
		"version: 1\ndefine:\n  contexts:\n    production:\n      fallback: pass\n      \
		 rules:\n        - classes: []\n          outcome: fail\n          facts: [fresh]\n",
		&[("define.contexts.production.rules[0].facts", 9)],
	);
}

#[test]
fn a_context_rule_points_at_its_own_line() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    default:\n      fallback: pass\n      \
		 rules:\n        - facts: [unclaimed]\n          outcome: notice\n        \
		 - facts: [unclaimed]\n          outcome: fail\n",
	);
	repo.write("docs/source-sentry.yaml", "root: true\n");
	repo.commit_at("2026-10-01T10:00:00Z");

	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let judgement =
		&document_or_unclaimed(&report, "docs/guide.md")["judgement"][0];
	assert_eq!(judgement["rule"]["outcome"], "fail", "{judgement}");
	assert_eq!(judgement["rule"]["origin"]["line"], 9, "{judgement}");
}

/// The subject of `path`, whatever its kind.
fn document_or_unclaimed<'a>(
	report: &'a serde_json::Value,
	path: &str,
) -> &'a serde_json::Value {
	report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject["document"] == path || subject["path"] == path)
		.unwrap_or_else(|| panic!("no subject {path} in {report}"))
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

#[test]
fn an_invalid_context_rule_points_at_its_own_line() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    default:\n      fallback: warn\n      \
		 rules:\n        - facts: [unclaimed]\n          outcome: warn\n        \
		 - facts: []\n          outcome: fail\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let diagnostic = &report["diagnostics"][0];
	assert_eq!(diagnostic["code"], "invalid-context", "{report}");
	assert_eq!(diagnostic["location"]["line"], 9, "{report}");
	assert_eq!(
		diagnostic["location"]["field"], "define.contexts.default.rules[1]",
		"{report}"
	);
}

#[test]
fn an_undeclared_class_on_file_points_at_its_line() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  classes:\n    alpha: {}\n",
	);
	repo.write(
		"docs/guide.md",
		"---\nsource-sentry:\n  classes:\n    - alpha\n    - missing\n---\n# Guide\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = repo.run(&["check", "--at", AT, "--format", "json"]).json();
	let diagnostic = report["diagnostics"]
		.as_array()
		.unwrap()
		.iter()
		.find(|diagnostic| diagnostic["code"] == "undeclared-class")
		.unwrap_or_else(|| panic!("an undeclared class: {report}"));
	assert_eq!(diagnostic["location"]["line"], 5, "{diagnostic}");
	assert_eq!(
		diagnostic["location"]["field"], "source-sentry.classes[1]",
		"{diagnostic}"
	);
}

/// Both report formats locate class and source diagnostics in a lockfile.
fn lockfile_diagnostics_point_at_their_lines(remote: bool) {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  classes:\n    alpha: {}\n",
	);
	let (file, guide, other) = if remote {
		(
			".source-sentry/remotes/team.lock",
			"abstract:guide.v1",
			"abstract:other.doc",
		)
	} else {
		("docs/source-sentry.lock", "guide.md", "other.md")
	};
	repo.write("docs/other.md", "# Other\n");
	repo.write(
		file,
		&format!(
			r#"{{
  "documents": {{
    "{guide}": {{
      "classes": [
        "alpha",
        "missing"
      ]
    }},
    "{other}": {{
      "sources": [
        {{
          "path": "/missing.rs"
        }},
        {{
          "pattern": "/../outside/*.rs"
        }}
      ]
    }}
  }},
  "retired": {{}},
  "schema": 1
}}"#
		),
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let text = repo.run(&["check", "--at", AT]);
	text.expect_code(3);
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(3);
	let report = run.json();
	for (code, key, suffix, line) in [
		("undeclared-class", guide, "classes[1]", 6),
		("missing-source", other, "sources[0].path", 12),
		("invalid-pattern", other, "sources[1].pattern", 15),
	] {
		let diagnostic = report["diagnostics"]
			.as_array()
			.unwrap()
			.iter()
			.find(|diagnostic| diagnostic["code"] == code)
			.unwrap_or_else(|| panic!("{code}: {report}"));
		assert_eq!(diagnostic["location"]["file"], file, "{diagnostic}");
		assert_eq!(diagnostic["location"]["line"], line, "{diagnostic}");
		assert_eq!(
			diagnostic["location"]["field"],
			format!("documents[{key:?}].{suffix}"),
			"{diagnostic}"
		);
		let diagnostic = text
			.stdout
			.lines()
			.find(|diagnostic| diagnostic.contains(code))
			.unwrap_or_else(|| panic!("{code}: {}", text.stdout));
		assert!(
			diagnostic.contains(&format!("{file}:{line}")),
			"{diagnostic}"
		);
	}
}

#[test]
fn local_lockfile_diagnostics_point_at_their_lines() {
	lockfile_diagnostics_point_at_their_lines(false);
}

#[test]
fn remote_lockfile_diagnostics_point_at_their_lines() {
	lockfile_diagnostics_point_at_their_lines(true);
}
