//! Tests of the report shapes, against `structure/report.example.json`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{
	CheckReport, ClockReport, DiffReport, OriginReport, scope, to_json,
};
use crate::claims::discover;
use crate::config::{ConfigSet, Origin, Position, Scope};
use crate::evaluate::{Evaluation, check};
use crate::model::RepoPath;
use crate::mutate::{Selection, diff};
use crate::policy::Threshold;
use crate::testing::{FakeHistory, FakeSnapshot, fake_oid, path};

/// The approved example of the check report.
const EXAMPLE: &str = include_str!("../../../structure/report.example.json");

/// The repository config: the example's `production` context, which blocks
/// privacy documents.
const REPOSITORY: &str = "\
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
          facts: ['stale', 'overdue', 'unattested', 'timing-unknown', \
          'upstream-stale']
          outcome: fail
";

/// The config of `docs/`, which sets the example's grace period.
const DOCS_CONFIG: &str = "schedule:\n  grace: 14\n";

/// The config of `audits/`, with a series due every two months.
const AUDITS_CONFIG: &str = "\
series:
  security-audits:
    pattern: 'audit-(?P<version>\\d+)\\.md'
    version-kind: integer
    lifecycle: records-only
    schedule:
      cadence:
        dtstart: 2026-01-15
        rrule: 'FREQ=MONTHLY;INTERVAL=2'
      grace: 14
";

fn instant(text: &str) -> DateTime<Utc> {
	text.parse().unwrap()
}

/// A lockfile holding `documents`.
fn lockfile(documents: &Value) -> Vec<u8> {
	serde_json::to_vec_pretty(
		&json!({"documents": documents, "retired": {}, "schema": 1}),
	)
	.unwrap()
}

/// The example's repository, evaluated at its instant under `production`:
///
/// - `docs/privacy.md` was attested against the first `db/schema.sql`,
///   which changed on 2026-10-20, so it is stale and, 14 days later,
///   overdue. Its second source is a pattern that matches nothing, which is
///   a warning.
/// - `docs/new-page.md` is unclaimed.
/// - `audits/` owes a security audit every two months. The only audit was
///   sealed on 2026-10-20, so the earlier windows are missed.
fn example_evaluation() -> Evaluation {
	let review = json!({"at": "2026-01-01T00:00:00Z", "by": null});
	let docs_lock = lockfile(&json!({
		"privacy.md": {
			"classes": ["privacy"],
			"owners": ["legal"],
			"sources": [
				{
					"oid": fake_oid(b"v1\n").to_string(),
					"path": "/db/schema.sql",
					"reviewed": review,
				},
				{"members": [], "pattern": "/db/legacy/*.sql"},
			],
		},
	}));
	let audits_lock = lockfile(&json!({
		"audit-1.md": {
			"seal": {
				"at": "2026-10-20T10:00:00Z",
				"by": null,
				"oid": fake_oid(b"audit 1\n").to_string(),
			},
		},
	}));
	let mut history = FakeHistory::new();
	history.commit(
		instant("2026-01-01T00:00:00Z"),
		&[
			(
				".source-sentry/source-sentry.yaml",
				Some(REPOSITORY.as_bytes()),
			),
			("audits/source-sentry.yaml", Some(AUDITS_CONFIG.as_bytes())),
			("db/schema.sql", Some(b"v1\n".as_slice())),
			("docs/new-page.md", Some(b"# New\n".as_slice())),
			("docs/privacy.md", Some(b"# Privacy\n".as_slice())),
			("docs/source-sentry.lock", Some(docs_lock.as_slice())),
			("docs/source-sentry.yaml", Some(DOCS_CONFIG.as_bytes())),
		],
	);
	history.commit(
		instant("2026-10-20T10:00:00Z"),
		&[
			("audits/audit-1.md", Some(b"audit 1\n".as_slice())),
			("audits/source-sentry.lock", Some(audits_lock.as_slice())),
			("db/schema.sql", Some(b"v2\n".as_slice())),
		],
	);
	let snapshot = FakeSnapshot::checkout(&history);
	let configs = ConfigSet::load(&snapshot).unwrap();
	let inventory = discover(&snapshot, &configs).unwrap();
	check(
		&snapshot,
		&history,
		&configs,
		&inventory,
		instant("2026-11-07T18:00:00Z"),
		Some("production"),
	)
	.unwrap()
}

/// The keys of a JSON value at every level. Arrays keep the distinct shapes
/// of their elements, and every value that is neither an object nor an
/// array is a leaf, so `null` and a string have the same shape.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
	Leaf,
	Array(BTreeSet<Shape>),
	Object(BTreeMap<String, Shape>),
}

fn shape(value: &Value) -> Shape {
	match value {
		Value::Object(object) => Shape::Object(
			object
				.iter()
				.map(|(key, value)| (key.clone(), shape(value)))
				.collect(),
		),
		Value::Array(items) => Shape::Array(items.iter().map(shape).collect()),
		Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
			Shape::Leaf
		}
	}
}

/// The subject of `report` whose `key` is `value`.
fn subject<'a>(report: &'a Value, key: &str, value: &str) -> &'a Value {
	report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| subject[key] == value)
		.unwrap_or_else(|| panic!("no subject with {key} {value}"))
}

/// A report built from an evaluation has the example's keys at
/// every level, for the report itself and for each kind of subject.
#[test]
fn the_report_has_the_keys_of_the_example() {
	let evaluation = example_evaluation();
	let report: Value = serde_json::from_str(
		&to_json(&CheckReport::new(&evaluation, Threshold::Warn)).unwrap(),
	)
	.unwrap();
	let example: Value = serde_json::from_str(EXAMPLE).unwrap();

	let without_subjects = |value: &Value| {
		let mut value = value.clone();
		value.as_object_mut().unwrap().remove("subjects");
		shape(&value)
	};
	assert_eq!(without_subjects(&report), without_subjects(&example));
	for (key, value) in [
		("document", "docs/privacy.md"),
		("path", "docs/new-page.md"),
		("name", "security-audits"),
	] {
		assert_eq!(
			shape(subject(&report, key, value)),
			shape(subject(&example, key, value)),
			"the subject with {key} {value} differs from the example"
		);
	}
}

/// The example's values that this evaluation reproduces exactly.
#[test]
fn the_report_reproduces_the_example_values() {
	let evaluation = example_evaluation();
	let report: Value = serde_json::from_str(
		&to_json(&CheckReport::new(&evaluation, Threshold::Warn)).unwrap(),
	)
	.unwrap();
	let example: Value = serde_json::from_str(EXAMPLE).unwrap();

	for key in ["at", "exit-code", "gates", "schema", "threshold"] {
		assert_eq!(report[key], example[key], "{key}");
	}
	assert_eq!(report["context"]["name"], "production");
	assert_eq!(report["context"]["origin"]["kind"], "config");
	assert_eq!(report["context"]["origin"]["scope"], "repository");
	assert_eq!(report["revision"]["kind"], "working-tree");

	let privacy = subject(&report, "document", "docs/privacy.md");
	let expected = subject(&example, "document", "docs/privacy.md");
	for key in ["declared-in", "facts", "field", "kind", "mode", "outcome"] {
		assert_eq!(privacy[key], expected[key], "{key}");
	}
	assert_eq!(privacy["deadline"], expected["deadline"]);
	assert_eq!(
		privacy["settings"]["classes"],
		expected["settings"]["classes"]
	);
	assert_eq!(
		privacy["settings"]["owners"],
		expected["settings"]["owners"]
	);
	assert_eq!(
		privacy["settings"]["cadence"],
		expected["settings"]["cadence"]
	);
	assert_eq!(privacy["settings"]["grace"]["value"], 14);
	assert_eq!(
		privacy["settings"]["grace"]["origin"]["scope"],
		"directory:docs"
	);
	assert_eq!(privacy["sources"][0]["state"], "drifted");
	assert_eq!(
		privacy["sources"][0]["clock"]["since"],
		"2026-10-20T10:00:00Z"
	);
	assert_eq!(
		privacy["judgement"][0]["rule"]["classes"],
		json!(["privacy"])
	);

	let unclaimed = subject(&report, "path", "docs/new-page.md");
	assert_eq!(unclaimed, subject(&example, "path", "docs/new-page.md"));

	let series = subject(&report, "name", "security-audits");
	assert_eq!(series["facts"], json!(["missed"]));
	assert_eq!(series["outcome"], "notice");
	assert_eq!(series["scope"], "directory:audits");
	assert_eq!(
		series["missed"].as_array().unwrap().last().unwrap(),
		&json!({"after": "2026-07-15", "due": "2026-09-15", "last-day": "2026-09-29"})
	);

	let [diagnostic] = report["diagnostics"].as_array().unwrap().as_slice()
	else {
		panic!("expected one diagnostic");
	};
	assert_eq!(diagnostic["code"], "empty-pattern");
	assert_eq!(diagnostic["severity"], "warning");
	assert_eq!(
		diagnostic["location"]["field"],
		"documents[\"privacy.md\"].sources[1].pattern"
	);
	assert_eq!(report["summary"]["outcomes"]["fail"], 1);
	assert_eq!(report["summary"]["outcomes"]["warn"], 1);
	assert_eq!(report["summary"]["outcomes"]["notice"], 1);
}

/// The continue gate passes on warnings alone, and the exit code follows
/// the threshold.
#[test]
fn the_threshold_sets_the_exit_code_but_not_the_gates() {
	let evaluation = example_evaluation();
	let report = CheckReport::new(&evaluation, Threshold::Fail);
	assert_eq!(report.exit_code, 2);
	assert_eq!(report.threshold, "fail");
	assert_eq!(
		(report.gates.report, report.gates.proceed),
		("fail", "fail")
	);
}

/// Keys are sorted at every level, because objects are serialized through
/// a sorted map. This fails if a dependency enables
/// `serde_json`'s `preserve_order`.
#[test]
fn json_objects_sort_their_keys() {
	let mut map = serde_json::Map::new();
	map.insert("summary".to_owned(), Value::Null);
	map.insert("at".to_owned(), Value::Null);
	map.insert("exit-code".to_owned(), Value::Null);
	assert_eq!(
		map.keys().map(String::as_str).collect::<Vec<_>>(),
		["at", "exit-code", "summary"]
	);
	let text =
		to_json(&CheckReport::new(&example_evaluation(), Threshold::Warn)).unwrap();
	let at = text.find("\"at\"").unwrap();
	let threshold = text.rfind("\"threshold\"").unwrap();
	assert!(at < threshold);
}

#[test]
fn clocks_are_objects_null_or_states() {
	let started = ClockReport::Started {
		commit: "sha1:1".to_owned(),
		since: "2026-01-01T00:00:00Z".to_owned(),
	};
	let written: Vec<Value> = [
		ClockReport::Idle,
		ClockReport::NotStarted,
		started,
		ClockReport::Unknown,
		ClockReport::Failed,
	]
	.iter()
	.map(|clock| serde_json::to_value(clock).unwrap())
	.collect();
	assert_eq!(
		written,
		[
			Value::Null,
			json!("not-started"),
			json!({"commit": "sha1:1", "since": "2026-01-01T00:00:00Z"}),
			json!("unknown"),
			json!("failed"),
		]
	);
}

#[test]
fn origins_and_scopes_are_written_in_the_report_format() {
	let origin = Origin {
		file: Some(path("docs/source-sentry.yaml")),
		position: Some(Position { line: 7, column: 3 }),
		import_chain: vec![path(".source-sentry/presets/base.yaml")],
		scope: Scope::Directory(path("docs")),
	};
	assert_eq!(
		serde_json::to_value(OriginReport::config(&origin)).unwrap(),
		json!({
			"file": "docs/source-sentry.yaml",
			"import-chain": [".source-sentry/presets/base.yaml"],
			"kind": "config",
			"line": 7,
			"scope": "directory:docs",
		})
	);
	assert_eq!(
		serde_json::to_value(OriginReport::config(&Origin::built_in())).unwrap(),
		json!({"kind": "built-in"})
	);
	assert_eq!(scope(&Scope::Repository), "repository");
	assert_eq!(scope(&Scope::Directory(RepoPath::ROOT)), "directory:.");
	assert_eq!(scope(&Scope::Remote("legal".to_owned())), "remote:legal");
}

/// The diff report: kebab-case keys, a schema, each entry's states and its
/// unified diff, and the expectations `attest` binds.
#[test]
fn the_diff_report_describes_each_entry() {
	let mut history = FakeHistory::new();
	history.commit(
		instant("2026-11-01T00:00:00Z"),
		&[
			("db/0.sql", Some(b"zero\n".as_slice())),
			("src/a.rs", Some(b"v1\n".as_slice())),
		],
	);
	let review = json!({"at": "2026-11-01T00:00:00Z", "by": null});
	let lock = lockfile(&json!({
		"guide.md": {"sources": [
			{"oid": fake_oid(b"v1\n").to_string(), "path": "/src/a.rs", "reviewed": review},
			{
				"members": [{
					"oid": fake_oid(b"zero\n").to_string(),
					"path": "/db/0.sql",
					"reviewed": review,
				}],
				"pattern": "/db/*.sql",
			},
		]},
	}));
	let snapshot = FakeSnapshot::checkout(&history)
		.with_file("src/a.rs", b"v2\n")
		.without_file("db/0.sql")
		.with_file("db/1.sql", &[0xff, 0xfe])
		.with_file("docs/guide.md", b"# Guide\n")
		.with_file("docs/source-sentry.lock", &lock);
	let configs = ConfigSet::load(&snapshot).unwrap();
	let inventory = discover(&snapshot, &configs).unwrap();
	let document = crate::model::DocumentId::Local(path("docs/guide.md"));
	let compared =
		diff(&snapshot, &history, &inventory, &document, &Selection::All).unwrap();
	let report = DiffReport::new(
		&compared,
		&crate::ports::Revision::WorkingTree {
			head: history.head(),
		},
		vec!["source-sentry".to_owned(), "attest".to_owned()],
		|old, new| format!("-{old}+{new}"),
	);
	let value: Value = serde_json::from_str(&to_json(&report).unwrap()).unwrap();

	assert_eq!(value["schema"], 1);
	assert_eq!(value["document"], "docs/guide.md");
	assert_eq!(value["attest"], json!(["source-sentry", "attest"]));
	assert_eq!(
		value["expectations"],
		json!([
			"db/0.sql@absent",
			format!("db/1.sql@{}", fake_oid(&[0xff, 0xfe])),
			format!("src/a.rs@{}", fake_oid(b"v2\n")),
		])
	);
	let path_source = &value["sources"][0];
	assert_eq!(path_source["declared"], "/src/a.rs");
	assert_eq!(path_source["pattern"], false);
	assert_eq!(
		path_source["entries"][0],
		json!({
			"baseline": {"oid": fake_oid(b"v1\n").to_string(), "state": "available"},
			"changed": true,
			"current": {"oid": fake_oid(b"v2\n").to_string(), "state": "present"},
			"diff": "-v1\n+v2\n",
			"path": "src/a.rs",
		})
	);
	let members = value["sources"][1]["entries"].as_array().unwrap();
	assert_eq!(
		members[0]["current"],
		json!({"oid": null, "state": "missing"})
	);
	assert_eq!(members[0]["diff"], "-zero\n+");
	assert_eq!(
		members[1]["baseline"],
		json!({"oid": null, "state": "unattested"})
	);
	assert_eq!(
		members[1]["diff"],
		Value::Null,
		"binary content has no diff"
	);
}
