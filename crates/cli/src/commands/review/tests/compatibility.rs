//! Sessions after inherited registration, repeated retirement and validation.

use serde_json::{Value, json};
use source_sentry_core::mutate::{
	Baseline, SourceSpec, Target, TrackRequest, TrackSummary, track,
};

use super::super::Answer;
use super::support::{Repo, Script, local};
use crate::workspace::load;

/// Registers a fixture document through the ordinary planner and backend.
fn register(
	repo: &Repo,
	document: &str,
	sources: Vec<SourceSpec>,
) -> TrackSummary {
	let workspace = repo.workspace();
	let snapshot = workspace.repository.working_tree_snapshot().unwrap();
	let (configs, inventory) = load(&workspace, &snapshot).unwrap();
	let planned = track(
		&snapshot,
		&configs,
		&inventory,
		&TrackRequest {
			document: local(document),
			target: Target::Lockfile { directory: None },
			placement_named: false,
			sources,
			classes: vec![],
			owners: vec![],
			schedule: None,
			url: None,
		},
	)
	.unwrap();
	workspace.repository.apply(&planned.plan).unwrap();
	planned.summary
}

/// Both series lifecycles queue copied declarations without predecessor reviews.
#[test]
fn an_inherited_series_entry_starts_unattested_and_is_queued() {
	for lifecycle in ["living", "records-only"] {
		let repo = Repo::new(json!({"audits/old/v2.md": {
			"owners": ["legal"],
			"schedule": {"grace": 7},
			"sources": [
				{"path": "../../src/a.rs"},
				{"pattern": "../../src/*.rs"}
			]
		}}));
		repo.write(
			".source-sentry/source-sentry.yaml",
			&format!(
				"version: 1\nseries:\n  audits:\n    pattern: 'audits/(?:[^/]+/)*v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: {lifecycle}\n"
			),
		);
		repo.attest("audits/old/v2.md");
		repo.commit();
		repo.write("audits/new/deep/v10.md", "# New audit\n");
		repo.git(&["add", "--", "audits/new/deep/v10.md"]);
		let registration = register(&repo, "audits/new/deep/v10.md", vec![]);
		assert_eq!(
			registration.registered.inherited.unwrap().predecessor,
			source_sentry_core::model::RepoPath::new("audits/old/v2.md").unwrap()
		);
		let entry = repo.entry("audits/new/deep/v10.md");
		assert_eq!(entry["owners"], json!(["legal"]));
		assert_eq!(entry["schedule"]["grace"], 7);
		for absent in ["reviewed", "seal", "completed"] {
			assert!(entry[absent].is_null());
		}
		assert_eq!(
			entry["sources"],
			json!([
				{"pattern": "../../../src/*.rs"},
				{"path": "../../../src/a.rs"}
			])
		);
		let mut script = Script::answers(&repo, &[Answer::Reread]);
		repo.run(&[], &mut script).unwrap();
		assert_eq!(script.shown, [local("audits/new/deep/v10.md")]);
		assert!(script.displays[0].contains("unattested"));
		assert!(script.comparisons[0].sources.iter().all(|source| {
			!source.entries.is_empty()
				&& source
					.entries
					.iter()
					.all(|entry| matches!(entry.baseline, Baseline::Unattested))
		}));
		assert!(script.summary().contains("0 documents still need review"));
		assert!(repo.entry("audits/new/deep/v10.md")["reviewed"].is_object());
	}
}

/// Earlier retirements never suppress a new registration or change its history.
#[test]
fn a_registered_path_with_several_tombstones_is_queued_normally() {
	let repo = Repo::new(json!({}));
	let retired = json!({"docs/a.md": [
		{"retired": {"at": "2026-10-02T10:00:00Z", "by": "First"}},
		{"retired": {"at": "2026-10-03T10:00:00Z", "by": "Second"}}
	]});
	repo.write("docs/a.md", "# New document\n");
	repo.write(
		"source-sentry.lock",
		&json!({"schema": 1, "documents": {}, "retired": retired}).to_string(),
	);
	repo.git(&["add", "--", "docs/a.md"]);
	register(
		&repo,
		"docs/a.md",
		vec![SourceSpec::Path("/src/a.rs".to_owned())],
	);
	let mut script = Script::answers(&repo, &[Answer::Reread]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md")]);
	assert!(script.displays[0].contains("unattested"));
	assert!(script.summary().contains("0 documents still need review"));
	let lockfile: Value =
		serde_json::from_str(&repo.read("source-sentry.lock")).unwrap();
	assert_eq!(lockfile["retired"], retired);
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_object());
}

/// Invalid cadences refuse the whole session, including declarations outside
/// its selection, sealed records and unused configuration.
#[test]
fn an_invalid_schedule_anywhere_refuses_before_a_prompt_or_write() {
	for placement in ["lockfile", "sealed", "remote", "on-file", "preset"] {
		let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
		let schedule = json!({"cadence": {
			"dtstart": "2026-01-02", "rrule": "FREQ=MONTHLY;BYMONTHDAY=1"
		}});
		let (file, content) = match placement {
			"remote" => (
				".source-sentry/remotes/legal.lock",
				json!({"schema": 1, "retired": {}, "documents": {
					"abstract:guide": {"schedule": schedule}
				}})
				.to_string(),
			),
			"on-file" => {
				repo.write("notes/source-sentry.yaml", "documents:\n  ignore: ['*']\n");
				(
					"notes/ignored.md",
					format!(
						"---\nsource-sentry: {}\n---\n# Ignored\n",
						json!({"schedule": schedule})
					),
				)
			}
			"preset" => (
				".source-sentry/presets/unused.yaml",
				json!({"schedule": schedule}).to_string(),
			),
			_ => {
				let mut entry = json!({"schedule": schedule});
				if placement == "sealed" {
					entry["seal"] = json!({
						"oid": format!("sha1:{}", repo.git(&["hash-object", "docs/b.md"])),
						"at": "2026-10-02T10:00:00Z", "by": "Earlier Reviewer"
					});
				}
				(
					"source-sentry.lock",
					json!({
						"schema": 1, "retired": {},
						"documents": {"docs/a.md": {}, "docs/b.md": entry}
					})
					.to_string(),
				)
			}
		};
		repo.write(file, &content);
		repo.git(&["add", "--all"]);
		let before = repo.read("source-sentry.lock");
		let mut script = Script::answers(&repo, &[Answer::Yes]);
		let error = repo.run(&["docs/a.md"], &mut script).unwrap_err();
		assert_eq!(error.code, 3, "{placement}: {error:?}");
		assert_eq!(
			error.message.matches("is not an occurrence").count(),
			1,
			"{placement}: {error:?}"
		);
		let location = repo
			.workspace()
			.display_path(&source_sentry_core::model::RepoPath::new(file).unwrap());
		assert!(error.message.contains(&format!("{location}:")));
		assert_eq!(script.shown, []);
		assert_eq!(script.messages, [] as [String; 0]);
		assert_eq!(repo.read("source-sentry.lock"), before);
		assert_eq!(repo.read(file), content);
	}
}
