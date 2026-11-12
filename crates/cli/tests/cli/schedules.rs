//! Schedule validation at every place that can write a cadence.

use std::fmt::Write as _;

use serde_json::json;

use crate::support::{AT, TestRepo, document};

/// Invalid cadences with distinct semantic failures.
const INVALID_RULES: [(&str, &str); 2] = [
	("FREQ=MONTHLY;BYMONTHDAY=1", "is not an occurrence"),
	("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30", "has no occurrence"),
];

/// A schedule with a cadence that starts on 2 January 2026.
fn schedule(rule: &str) -> String {
	format!(
		"schedule:\n  grace: 5\n  cadence:\n    dtstart: 2026-01-02\n    rrule: '{rule}'\n"
	)
}

/// Indents each line of a YAML fixture.
fn indent(text: &str, prefix: &str) -> String {
	let mut indented = String::new();
	for line in text.lines() {
		let _ = writeln!(indented, "{prefix}{line}");
	}
	indented
}

/// Invalid raw recurrence tokens still produce a complete JSON diagnostic.
#[test]
fn malformed_recurrence_tokens_give_configuration_reports() {
	for rule in [
		"FREQ=MONTHLY;BYDAY=日",
		"FREQ=MONTHLY;BYDAY=💥",
		"FREQ=MONTHLY;BYDAY=garbageSU",
		"FREQ=MONTHLY;BYDAY=999999999999SU",
		"FREQ=DAILY;BY\u{017f}ECOND=0",
		"FREQ=DAILY;UNT\u{0131}L=20271201T000000Z",
		"FREQ=MONTHLY;BYMONTHDAY=0",
		"FREQ=MONTHLY;BYDAY=0SU",
		"FREQ=DAILY;BYHOUR=",
		"FREQ=DAILY;BYMINUTE=",
		"FREQ=DAILY;BYSECOND=",
		"FREQ=MONTHLY;BYMONTHDAY=",
		"FREQ=DAILY;",
	] {
		let repo = TestRepo::init();
		let file = ".source-sentry/source-sentry.yaml";
		repo.write(
			file,
			&format!(
				"version: 1\nschedule:\n  cadence:\n    dtstart: '2026-11-01'\n    rrule: '{rule}'\n"
			),
		);
		assert_invalid_schedule(&repo, file, "schedule.cadence", 3, rule);
	}
}

/// Invalid raw rules are refused before creating or extending a declaration.
#[test]
fn malformed_recurrence_tokens_are_never_written() {
	let repo = TestRepo::init();
	repo.commit_at("2026-10-01T10:00:00Z");
	for existing in [false, true] {
		if existing {
			repo
				.run(&["track", "docs/guide.md", "--yes"])
				.expect_code(0);
		}
		let before = existing.then(|| repo.read("docs/source-sentry.lock"));
		let index = repo.git(&["write-tree"]);
		for rule in [
			"FREQ=MONTHLY;BYDAY=日",
			"FREQ=MONTHLY;BYDAY=garbageSU",
			"FREQ=DAILY;BY\u{017f}ECOND=0",
			"FREQ=DAILY;UNT\u{0131}L=20271201T000000Z",
			"FREQ=MONTHLY;BYMONTHDAY=0",
			"FREQ=MONTHLY;BYDAY=0SU",
			"FREQ=DAILY;BYHOUR=",
			"FREQ=DAILY;BYMINUTE=",
			"FREQ=DAILY;BYSECOND=",
			"FREQ=MONTHLY;BYMONTHDAY=",
			"FREQ=DAILY;",
		] {
			let run = repo.run(&[
				"track",
				"docs/guide.md",
				"--cadence",
				"2026-11-01",
				rule,
				"--yes",
			]);
			run.expect_code(2);
			assert!(run.stderr.contains(rule), "{}", run.stderr);
			assert_eq!(repo.git(&["write-tree"]), index);
			if let Some(before) = &before {
				assert_eq!(&repo.read("docs/source-sentry.lock"), before);
			} else {
				assert!(!repo.exists("docs/source-sentry.lock"));
			}
		}
	}
}

/// Checks both snapshots and human output for one error at its written line.
fn assert_invalid_schedule(
	repo: &TestRepo,
	file: &str,
	field: &str,
	line: u64,
	reason: &str,
) {
	repo.commit_at("2026-10-01T10:00:00Z");
	for revision in [&[][..], &["--rev", "HEAD"][..]] {
		let mut args = vec!["check", "--at", AT, "--format", "json"];
		args.extend_from_slice(revision);
		let run = repo.run(&args);
		run.expect_code(3);
		let report = run.json();
		let diagnostics = report["diagnostics"].as_array().unwrap();
		assert_eq!(diagnostics.len(), 1, "{report:#}");
		let diagnostic = &diagnostics[0];
		assert_eq!(diagnostic["code"], "invalid-schedule", "{report:#}");
		assert_eq!(diagnostic["severity"], "configuration-error");
		assert_eq!(diagnostic["location"]["file"], file);
		assert_eq!(diagnostic["location"]["field"], field);
		assert_eq!(diagnostic["location"]["line"], line, "{report:#}");
		assert!(
			diagnostic["message"].as_str().unwrap().contains(reason),
			"{diagnostic:#}"
		);
	}
	let human = repo.run(&["check", "--at", AT]);
	human.expect_code(3);
	let output = format!("{}{}", human.stdout, human.stderr);
	assert!(output.contains(&format!("{file}:{line}")), "{output}");
	assert!(output.contains(reason), "{output}");
}

/// Repository defaults are checked without a document or scanning root.
#[test]
fn an_unused_repository_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/source-sentry.yaml";
		repo.write(file, &format!("version: 1\n{}", schedule(rule)));
		assert_invalid_schedule(&repo, file, "schedule.cadence", 4, reason);
	}
}

/// An empty directory still validates its defaults.
#[test]
fn an_unused_directory_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = "empty/source-sentry.yaml";
		repo.write(file, &schedule(rule));
		assert_invalid_schedule(&repo, file, "schedule.cadence", 3, reason);
	}
}

/// Imported presets are checked even when later settings replace them.
#[test]
fn an_imported_overridden_preset_schedule_is_validated_once() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/presets/shared.yaml";
		repo.write(file, &schedule(rule));
		for directory in ["docs", "empty"] {
			repo.write(
				&format!("{directory}/source-sentry.yaml"),
				"imports: [shared, shared]\nschedule:\n  cadence: null\n",
			);
		}
		assert_invalid_schedule(&repo, file, "schedule.cadence", 3, reason);
	}
}

/// Presets do not need to be imported to be validated.
#[test]
fn an_unimported_preset_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/presets/unused.yaml";
		repo.write(file, &schedule(rule));
		assert_invalid_schedule(&repo, file, "schedule.cadence", 3, reason);
	}
}

/// An owner config does not need a paired lockfile to be validated.
#[test]
fn an_unused_remote_owner_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/remotes/legal.yaml";
		repo.write(file, &schedule(rule));
		assert_invalid_schedule(&repo, file, "schedule.cadence", 3, reason);
	}
}

/// A series in an unimported preset is checked without members or a scope.
#[test]
fn an_unused_series_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/presets/unused.yaml";
		let schedule = indent(&schedule(rule), "    ");
		repo.write(
			file,
			&format!(
				"series:\n  logs:\n    pattern: '(?P<version>\\d+)\\.md'\n    \
				 version-kind: integer\n    lifecycle: records-only\n{schedule}"
			),
		);
		assert_invalid_schedule(
			&repo,
			file,
			"series.logs.schedule.cadence",
			8,
			reason,
		);
	}
}

/// A lockfile declaration reports its own cadence line.
#[test]
fn a_lockfile_schedule_is_validated_at_its_written_line() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = "docs/source-sentry.lock";
		repo.write(
			file,
			&serde_json::to_string_pretty(&json!({
				"schema": 1,
				"documents": {"guide.md": {"schedule": {"cadence": {
					"dtstart": "2026-01-02", "rrule": rule
				}}}},
				"retired": {}
			}))
			.unwrap(),
		);
		assert_invalid_schedule(
			&repo,
			file,
			"documents[\"guide.md\"].schedule.cadence",
			5,
			reason,
		);
	}
}

/// Ignoring an on-file document does not suppress its schedule validation.
#[test]
fn an_on_file_schedule_is_validated_at_its_written_line() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = "docs/guide.md";
		repo.write("docs/source-sentry.yaml", "documents:\n  ignore: ['*']\n");
		let schedule = indent(&schedule(rule), "  ");
		repo.write(
			file,
			&format!("---\nsource-sentry:\n{schedule}---\n# Guide\n"),
		);
		assert_invalid_schedule(
			&repo,
			file,
			"source-sentry.schedule.cadence",
			5,
			reason,
		);
	}
}

/// A sealed declaration is validated although records use no deadline.
#[test]
fn a_sealed_records_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = "docs/source-sentry.lock";
		let oid = repo.git(&["hash-object", "docs/guide.md"]);
		repo.write(
			file,
			&serde_json::to_string_pretty(&json!({
				"schema": 1,
				"documents": {"guide.md": {
					"schedule": {"cadence": {
						"dtstart": "2026-01-02", "rrule": rule
					}},
					"seal": {
						"at": "2026-10-01T10:00:00Z", "by": null,
						"oid": format!("sha1:{oid}")
					}
				}},
				"retired": {}
			}))
			.unwrap(),
		);
		assert_invalid_schedule(
			&repo,
			file,
			"documents[\"guide.md\"].schedule.cadence",
			5,
			reason,
		);
	}
}

/// Remote declarations use the same validation and locations as local ones.
#[test]
fn a_remote_declarations_schedule_is_validated() {
	for (rule, reason) in INVALID_RULES {
		let repo = TestRepo::init();
		let file = ".source-sentry/remotes/legal.lock";
		repo.write(
			file,
			&serde_json::to_string_pretty(&json!({
				"schema": 1,
				"documents": {"abstract:guide": {"schedule": {"cadence": {
					"dtstart": "2026-01-02", "rrule": rule
				}}}},
				"retired": {}
			}))
			.unwrap(),
		);
		assert_invalid_schedule(
			&repo,
			file,
			"documents[\"abstract:guide\"].schedule.cadence",
			5,
			reason,
		);
	}
}

/// Identical invalid values written twice produce two located errors.
#[test]
fn each_written_schedule_is_reported_once() {
	let repo = TestRepo::init();
	repo.write("docs/other.md", "# Other\n");
	let declaration = json!({"schedule": {"cadence": {
		"dtstart": "2026-01-02", "rrule": INVALID_RULES[0].0
	}}});
	repo.write(
		"docs/source-sentry.lock",
		&serde_json::to_string_pretty(&json!({
			"schema": 1,
			"documents": {
				"guide.md": declaration, "other.md": declaration
			},
			"retired": {}
		}))
		.unwrap(),
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(3);
	let report = run.json();
	let diagnostics = report["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 2, "{report:#}");
	for (diagnostic, line) in diagnostics.iter().zip([5, 13]) {
		assert_eq!(diagnostic["code"], "invalid-schedule");
		assert_eq!(diagnostic["location"]["line"], line, "{report:#}");
	}
}

/// Grace, cadence and time zone remain effective after separate inheritance.
#[test]
fn a_valid_schedule_resolves_inherited_grace_and_time_zone() {
	let repo = TestRepo::init();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ntime-zone: America/New_York\n",
	);
	repo.write("docs/source-sentry.yaml", &schedule("FREQ=DAILY"));
	repo.write(
		"docs/guide.md",
		"---\nsource-sentry:\n  schedule:\n    grace: 1\n---\n# Guide\n",
	);
	repo.commit_at("2026-10-01T10:00:00Z");
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(1);
	let report = run.json();
	assert_eq!(report["diagnostics"], json!([]));
	assert_eq!(
		document(&report, "docs/guide.md")["deadline"],
		json!({"due": "2026-10-02", "overdue-from": "2026-10-03T04:00:00Z"})
	);
}

/// A valid cadence can still lack a due date for a resolved clock.
#[test]
fn a_resolved_schedule_without_a_required_due_date_is_rejected() {
	let repo = TestRepo::init();
	let file = "docs/source-sentry.yaml";
	repo.write(file, &schedule("FREQ=YEARLY;INTERVAL=500"));
	repo.write(
		"docs/guide.md",
		"---\nsource-sentry:\n  schedule:\n    grace: 1\n---\n# Guide\n",
	);
	assert_invalid_schedule(
		&repo,
		file,
		"schedule.cadence",
		3,
		"has no occurrence",
	);
}

/// A cadence's evaluation errors keep an on-file declaration's own line.
#[test]
fn a_resolved_declaration_reports_its_cadence_line() {
	let repo = TestRepo::init();
	let schedule = indent(&schedule("FREQ=YEARLY;INTERVAL=500"), "  ");
	repo.write(
		"docs/guide.md",
		&format!("---\nsource-sentry:\n{schedule}---\n# Guide\n"),
	);
	assert_invalid_schedule(
		&repo,
		"docs/guide.md",
		"source-sentry.schedule.cadence",
		5,
		"has no occurrence",
	);
}

/// Different document clocks still produce one error per written cadence.
#[test]
fn a_resolved_inherited_schedule_is_reported_once() {
	let repo = TestRepo::init();
	repo.write(
		"docs/source-sentry.yaml",
		&schedule("FREQ=YEARLY;INTERVAL=500"),
	);
	repo.write("docs/guide.md", "---\nsource-sentry: {}\n---\n# Guide\n");
	repo.commit_at("2026-10-01T10:00:00Z");
	repo.write("docs/other.md", "---\nsource-sentry: {}\n---\n# Other\n");
	repo.commit_at("2026-10-02T10:00:00Z");
	let run = repo.run(&["check", "--at", AT, "--format", "json"]);
	run.expect_code(3);
	let report = run.json();
	let diagnostics = report["diagnostics"].as_array().unwrap();
	assert_eq!(diagnostics.len(), 1, "{report:#}");
	assert_eq!(diagnostics[0]["code"], "invalid-schedule");
	assert_eq!(diagnostics[0]["location"]["line"], 3);
	assert_eq!(
		diagnostics[0]["location"]["file"],
		"docs/source-sentry.yaml"
	);
}

/// An imported series keeps its cadence's own line for evaluation errors.
#[test]
fn a_resolved_series_schedule_reports_its_cadence_line() {
	let repo = TestRepo::init();
	let file = ".source-sentry/presets/logs.yaml";
	let schedule = indent(&schedule("FREQ=YEARLY;INTERVAL=500"), "    ");
	repo.write(
		file,
		&format!(
			"series:\n  logs:\n    pattern: '(?P<version>\\d+)\\.md'\n    \
			 version-kind: integer\n    lifecycle: records-only\n{schedule}"
		),
	);
	repo.write("docs/source-sentry.yaml", "imports: [logs]\n");
	assert_invalid_schedule(
		&repo,
		file,
		"series.logs.schedule.cadence",
		8,
		"has no occurrence",
	);
}
