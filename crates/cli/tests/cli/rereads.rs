//! Reread schedules: a deadline on rereading a whole document, which keeps a
//! document honest when none of its sources can change.

use serde_json::{Value, json};

use crate::support::{AT, TestRepo, document};

/// A reread schedule on the first of every month, from 1 January 2026.
fn monthly() -> Value {
	json!({"dtstart": "2026-01-01", "rrule": "FREQ=MONTHLY;BYMONTHDAY=1"})
}

/// Writes the project config with `extra` after its version, and a
/// lockfile at the repository root that tracks `docs/host.md` with `entry`
/// as its declaration. `docs/guide.md` is removed, so that nothing else is
/// reported.
fn host(repo: &TestRepo, extra: &str, entry: &Value) {
	repo.git(&["rm", "--quiet", "--cached", "docs/guide.md"]);
	std::fs::remove_file(repo.path().join("docs/guide.md")).unwrap();
	repo.write(
		".source-sentry/source-sentry.yaml",
		&format!("version: 1\n{extra}"),
	);
	repo.write("docs/host.md", "# Host\n");
	lockfile(repo, &json!({"docs/host.md": entry}));
}

/// Writes the lockfile at the repository root with `documents`.
fn lockfile(repo: &TestRepo, documents: &Value) {
	let lockfile = json!({"documents": documents, "retired": {}, "schema": 1});
	repo.write(
		"source-sentry.lock",
		&serde_json::to_string_pretty(&lockfile).unwrap(),
	);
}

/// The declaration of a document without sources, reread at `at`, with
/// `schedule` as its own schedule.
fn reread_at(at: &str, schedule: &Value) -> Value {
	json!({
		"reviewed": {"at": at, "by": "QA"},
		"schedule": schedule,
	})
}

/// The JSON report of `check --at <at>`, which must exit with `code`.
fn report(repo: &TestRepo, at: &str, code: i32) -> Value {
	let run = repo.run(&["check", "--at", at, "--format", "json"]);
	run.expect_code(code);
	run.json()
}

/// The facts of `docs/host.md` in `report`.
fn facts(report: &Value) -> Vec<&str> {
	document(report, "docs/host.md")["facts"]
		.as_array()
		.unwrap()
		.iter()
		.map(|fact| fact.as_str().unwrap())
		.collect()
}

/// A document without sources, reread on 5 August and due again on the
/// first of every month, is overdue in November: its reread was due on 1
/// September. It is not stale, as nothing in it changed, and the default
/// context warns.
#[test]
fn a_document_without_sources_is_overdue_for_its_reread() {
	let repo = TestRepo::init();
	let schedule = json!({"reread": monthly()});
	host(&repo, "", &reread_at("2026-08-05T10:00:00Z", &schedule));
	repo.commit_at("2026-08-05T12:00:00Z");

	let report = report(&repo, AT, 1);
	assert_eq!(facts(&report), ["reread-due", "reread-overdue"]);
	let host = document(&report, "docs/host.md");
	assert_eq!(host["outcome"], "warn");
	assert_eq!(host["deadline"]["kind"], "reread");
	assert_eq!(host["deadline"]["due"], "2026-09-01");
	assert_eq!(host["deadline"]["overdue-from"], "2026-09-02T00:00:00Z");
	assert_eq!(host["settings"]["reread"]["value"], monthly());

	let human = repo.run(&["check", "--at", AT]);
	human.expect_code(1);
	assert!(
		human
			.stdout
			.contains("reread-overdue (missed reread by 2026-09-01)"),
		"{}",
		human.stdout
	);
}

/// A reread schedule in YAML, monthly from 1 January 2026, at `indent`
/// spaces.
fn monthly_yaml(indent: usize) -> String {
	let pad = " ".repeat(indent);
	format!(
		"{pad}reread:\n{pad}  dtstart: 2026-01-01\n{pad}  rrule: 'FREQ=MONTHLY'\n"
	)
}

/// The current instant, for checks after a review recorded now.
fn now() -> String {
	jiff::Timestamp::now().to_string()
}

/// `check` and `explain` print the command that clears a document whose
/// only pending work is a reread, and running it does.
#[test]
fn the_command_to_copy_records_a_reread() {
	let repo = TestRepo::init();
	let schedule = json!({"reread": monthly()});
	host(&repo, "", &reread_at("2026-08-05T10:00:00Z", &schedule));
	repo.commit_at("2026-08-05T12:00:00Z");

	let command = "source-sentry attest docs/host.md --reread";
	let check = repo.run(&["check", "--at", AT]);
	check.expect_code(1);
	assert!(check.stdout.contains(command), "{}", check.stdout);
	let explain = repo.run(&["explain", "docs/host.md", "--at", AT]);
	explain.expect_code(0);
	assert!(explain.stdout.contains(command), "{}", explain.stdout);
	assert!(
		explain
			.stdout
			.contains("last reread at 2026-08-05 10:00 UTC by QA"),
		"{}",
		explain.stdout
	);
	assert!(
		explain.stdout.contains("reread by 2026-09-01"),
		"{}",
		explain.stdout
	);

	repo
		.run(&["attest", "docs/host.md", "--reread", "--yes"])
		.expect_code(0);
	let report = report(&repo, &now(), 0);
	assert_eq!(facts(&report), ["fresh"]);
}

/// Attesting the sources of a document leaves its last reread, and so its
/// reread deadline, as they were, and the command to copy is not printed
/// while a source is stale.
#[test]
fn attesting_the_sources_does_not_reset_the_reread() {
	let repo = TestRepo::init();
	let mut entry =
		reread_at("2026-08-05T10:00:00Z", &json!({"reread": monthly()}));
	entry["sources"] = json!([{"path": "/src/a.rs"}]);
	host(&repo, "", &entry);
	repo.commit_at("2026-08-05T12:00:00Z");
	let stale = repo.run(&["check", "--at", AT]);
	stale.expect_code(1);
	assert!(!stale.stdout.contains("--reread"), "{}", stale.stdout);

	repo
		.run(&["attest", "docs/host.md", "--yes"])
		.expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read("source-sentry.lock")).unwrap();
	let declaration = &lockfile["documents"]["docs/host.md"];
	assert_eq!(declaration["reviewed"]["at"], "2026-08-05T10:00:00Z");
	assert!(declaration["sources"][0]["oid"].is_string());

	let report = report(&repo, &now(), 1);
	assert_eq!(facts(&report), ["reread-due", "reread-overdue"]);
}

/// A reread schedule is inherited from a directory config and from a
/// preset, each named as its origin, and `reread: null` removes an
/// inherited one. A cadence alone never makes a document without sources
/// due.
#[test]
fn reread_schedules_are_inherited_and_removed_like_cadences() {
	let repo = TestRepo::init();
	let reviewed = json!({"at": "2026-08-05T10:00:00Z", "by": "QA"});
	host(&repo, "", &json!({"reviewed": reviewed}));
	repo.write(
		"docs/source-sentry.yaml",
		&format!("schedule:\n{}", monthly_yaml(2)),
	);
	repo.write(
		".source-sentry/presets/rereads.yaml",
		&format!("schedule:\n{}", monthly_yaml(2)),
	);
	repo.write("hosts/source-sentry.yaml", "imports: [rereads]\n");
	repo.write("hosts/a.md", "# A\n");
	repo.write("plain/b.md", "# B\n");
	repo.write(
		"plain/source-sentry.yaml",
		"schedule:\n  cadence:\n    dtstart: 2026-01-01\n    rrule: 'FREQ=MONTHLY'\n",
	);
	repo.write("removed/c.md", "# C\n");
	repo.write("removed/source-sentry.yaml", "imports: [rereads]\n");
	lockfile(
		&repo,
		&json!({
			"docs/host.md": {"reviewed": reviewed},
			"hosts/a.md": {"reviewed": reviewed},
			"plain/b.md": {"reviewed": reviewed},
			"removed/c.md": {"reviewed": reviewed, "schedule": {"reread": null}},
		}),
	);
	repo.commit_at("2026-08-05T12:00:00Z");

	let report = report(&repo, AT, 1);
	let overdue = json!(["reread-due", "reread-overdue"]);
	let host = document(&report, "docs/host.md");
	assert_eq!(host["facts"], overdue);
	assert_eq!(
		host["settings"]["reread"]["origin"],
		json!({
			"file": "docs/source-sentry.yaml",
			"import-chain": [],
			"kind": "config",
			"line": 2,
			"scope": "directory:docs",
		})
	);
	let imported = document(&report, "hosts/a.md");
	assert_eq!(imported["facts"], overdue);
	assert_eq!(
		imported["settings"]["reread"]["origin"]["file"],
		".source-sentry/presets/rereads.yaml"
	);
	assert_eq!(
		imported["settings"]["reread"]["origin"]["import-chain"],
		json!(["hosts/source-sentry.yaml"])
	);

	let cadence = document(&report, "plain/b.md");
	assert_eq!(cadence["facts"], json!(["fresh"]));
	assert_eq!(cadence["settings"]["reread"]["value"], Value::Null);
	assert!(cadence["settings"]["cadence"]["value"].is_object());

	let removed = document(&report, "removed/c.md");
	assert_eq!(removed["facts"], json!(["fresh"]));
	assert_eq!(removed["settings"]["reread"]["value"], Value::Null);
	assert_eq!(
		removed["settings"]["reread"]["origin"],
		json!({
			"field": "documents[\"removed/c.md\"].schedule.reread",
			"file": "source-sentry.lock",
			"kind": "declaration",
		})
	);
}

/// A series cannot have its own reread schedule, since its cadence already
/// asks for a new entry each window. The error points to the config's own
/// schedule.
#[test]
fn a_reread_schedule_in_a_series_is_a_configuration_error() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		&format!(
			"version: 1\nseries:\n  logs:\n    pattern: 'logs/(?P<version>\\d+)\\.md'\n    \
			 version-kind: integer\n    lifecycle: living\n    schedule:\n{}",
			monthly_yaml(6)
		),
	);
	repo.commit_at("2026-10-01T10:00:00Z");

	let report = report(&repo, AT, 3);
	let diagnostics = report["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 1, "{report:#}");
	assert_eq!(diagnostics[0]["code"], "invalid-config");
	assert_eq!(
		diagnostics[0]["location"]["field"],
		"series.logs.schedule.reread"
	);
	assert_eq!(diagnostics[0]["location"]["line"], 8);
	let message = diagnostics[0]["message"].as_str().unwrap();
	assert!(message.contains("top-level `schedule`"), "{message}");
}

/// An invalid reread schedule is `invalid-schedule` at its own field,
/// whether a config or a declaration writes it, and whether or not any
/// document uses it.
#[test]
fn an_invalid_reread_schedule_is_reported_at_its_field() {
	let rule =
		json!({"dtstart": "2026-01-02", "rrule": "FREQ=MONTHLY;BYMONTHDAY=1"});
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\nschedule:\n  reread:\n    dtstart: 2026-01-02\n    \
		 rrule: 'FREQ=MONTHLY;BYMONTHDAY=1'\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let config = report(&repo, AT, 3);
	let diagnostics = config["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 1, "{config:#}");
	assert_eq!(diagnostics[0]["code"], "invalid-schedule");
	assert_eq!(diagnostics[0]["location"]["field"], "schedule.reread");
	assert_eq!(diagnostics[0]["location"]["line"], 3);

	let repo = TestRepo::init();
	host(&repo, "", &json!({"schedule": {"reread": rule}}));
	repo.commit_at("2026-10-01T10:00:00Z");
	let report = report(&repo, AT, 3);
	let found: Vec<(&Value, &Value)> = report["diagnostics"]
		.as_array()
		.unwrap()
		.iter()
		.map(|diagnostic| (&diagnostic["code"], &diagnostic["location"]["field"]))
		.collect();
	assert_eq!(
		found,
		[(
			&json!("invalid-schedule"),
			&json!("documents[\"docs/host.md\"].schedule.reread")
		)]
	);
}

/// A context may name the reread facts: here a reread that is overdue
/// fails the build, while one that is only due is left to the fallback.
#[test]
fn a_context_rule_can_fail_an_overdue_reread() {
	let repo = TestRepo::init();
	host(
		&repo,
		"define:\n  contexts:\n    production:\n      fallback: pass\n      rules:\n        \
		 - facts: ['reread-overdue']\n          outcome: fail\n",
		&reread_at("2026-08-05T10:00:00Z", &json!({"reread": monthly()})),
	);
	repo.commit_at("2026-08-05T12:00:00Z");

	let run = repo.run(&["check", "--at", AT, "--context", "production"]);
	run.expect_code(2);
	assert!(
		run.stdout.contains("production: reread-overdue → fail"),
		"{}",
		run.stdout
	);
	repo
		.run(&[
			"check",
			"--at",
			"2026-09-01T12:00:00Z",
			"--context",
			"production",
		])
		.expect_code(0);
}

/// `track --reread` writes a reread schedule as `--cadence` writes a
/// cadence, in a lockfile or in frontmatter, also to a tracked document,
/// and refuses an invalid rule without writing anything.
#[test]
fn track_writes_a_reread_schedule() {
	let repo = TestRepo::init();
	repo.commit_at("2026-08-05T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--reread",
			"2026-01-01",
			"FREQ=MONTHLY",
			"--yes",
		])
		.expect_code(0);
	let lockfile: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lockfile["documents"]["guide.md"]["schedule"],
		json!({"reread": {"dtstart": "2026-01-01", "rrule": "FREQ=MONTHLY"}})
	);

	repo.write("docs/other.md", "# Other\n");
	repo
		.run(&["track", "docs/other.md", "--on-file", "--yes"])
		.expect_code(0);
	repo
		.run(&[
			"track",
			"docs/other.md",
			"--reread",
			"2026-01-05",
			"FREQ=WEEKLY",
			"--yes",
		])
		.expect_code(0);
	let other = repo.read("docs/other.md");
	assert!(
		other.contains("  schedule:\n    reread:\n      dtstart: 2026-01-05\n"),
		"{other}"
	);

	let before = repo.read("docs/source-sentry.lock");
	let refused = repo.run(&[
		"track",
		"docs/guide.md",
		"--reread",
		"2026-01-02",
		"FREQ=MONTHLY;BYMONTHDAY=1",
		"--yes",
	]);
	refused.expect_code(2);
	assert!(
		refused.stderr.contains("is not an occurrence"),
		"{}",
		refused.stderr
	);
	assert_eq!(repo.read("docs/source-sentry.lock"), before);
}

/// A reread dated after the check, on a document with a reread schedule, is
/// the configuration error `future-reread` at its `reviewed` field.
#[test]
fn a_reread_dated_after_the_check_is_a_configuration_error() {
	let repo = TestRepo::init();
	host(
		&repo,
		"",
		&reread_at("2026-12-01T10:00:00Z", &json!({"reread": monthly()})),
	);
	repo.commit_at("2026-08-05T12:00:00Z");

	let report = report(&repo, AT, 3);
	let diagnostics = report["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 1, "{report:#}");
	assert_eq!(diagnostics[0]["code"], "future-reread");
	assert_eq!(diagnostics[0]["severity"], "configuration-error");
	assert_eq!(
		diagnostics[0]["location"]["field"],
		"documents[\"docs/host.md\"].reviewed"
	);
}
