//! Document history follows explicit renames, including in exported projects.

use serde_json::{Value, json};

use crate::support::TestRepo;

/// The instant after all fixture commits.
const AT: &str = "2026-11-12T20:00:00Z";

/// The document subject named by its project-relative path.
fn document(report: &Value, path: &str) -> Value {
	report["subjects"]
		.as_array()
		.unwrap()
		.iter()
		.find(|subject| {
			subject["kind"] == "document" && subject["document"] == path
		})
		.unwrap()
		.clone()
}

/// An overdue unattested declaration retains its original clock through two
/// moves, before and after each commit, even when its original path is reused.
#[test]
fn rename_history_keeps_obligation_clocks_through_a_chain() {
	let repo = TestRepo::init();
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\ndefine:\n  contexts:\n    default:\n      fallback: notice\n      rules:\n        - facts: [overdue]\n          outcome: fail\n");
	repo.commit_at("2026-01-01T12:00:00Z");
	repo
		.run(&[
			"track",
			"docs/guide.md",
			"--source",
			"/src/a.rs",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");
	let before = repo
		.run(&["check", "--format", "json", "--at", "2026-02-10T12:00:00Z"])
		.expect_code(2)
		.json();
	let before = document(&before, "docs/guide.md");
	for (old, new, date) in [
		("docs/guide.md", "docs/second.md", "2026-02-01T12:00:00Z"),
		("docs/second.md", "docs/third.md", "2026-02-05T12:00:00Z"),
	] {
		repo.git(&["mv", old, new]);
		repo.run(&["rename", old, new, "--yes"]).expect_code(0);
		for committed in [false, true] {
			if committed {
				repo.commit_at(date);
			}
			let report = repo
				.run(&["check", "--format", "json", "--at", "2026-02-10T12:00:00Z"])
				.expect_code(2)
				.json();
			let after = document(&report, new);
			assert_eq!(after["unattested"], before["unattested"]);
			assert_eq!(after["deadline"], before["deadline"]);
		}
	}
	let lock: Value =
		serde_json::from_str(&repo.read("docs/source-sentry.lock")).unwrap();
	assert_eq!(
		lock["documents"]["third.md"]["renamed"][0]["from"],
		"/docs/guide.md"
	);
	assert_eq!(
		lock["documents"]["third.md"]["renamed"][1]["from"],
		"/docs/second.md"
	);
	repo.write("docs/guide.md", "# Another document\n");
	repo
		.run(&["track", "docs/guide.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-02-06T12:00:00Z");
	let report = repo
		.run(&["check", "--format", "json", "--at", "2026-02-10T12:00:00Z"])
		.expect_code(2)
		.json();
	assert_eq!(
		document(&report, "docs/third.md")["unattested"],
		before["unattested"]
	);
}

/// A seal and an on-file living completion keep the same cadence credit
/// after a move. On-file metadata changes, but its completion does not.
#[test]
fn rename_history_keeps_sealed_and_living_series_credit() {
	for lifecycle in ["records-only", "living"] {
		let repo = TestRepo::init();
		repo.write(".source-sentry/source-sentry.yaml", &format!("version: 1\nseries:\n  audits:\n    pattern: 'audits/(?:old|new)/v(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: {lifecycle}\n    schedule:\n      cadence:\n        dtstart: 2026-10-02\n        rrule: FREQ=MONTHLY\n"));
		repo.write("audits/old/v1.md", "# Audit\n");
		let placement = if lifecycle == "living" {
			"--on-file"
		} else {
			"--lockfile"
		};
		repo
			.run(&["track", "audits/old/v1.md", placement, "--yes"])
			.expect_code(0);
		let action = if lifecycle == "living" {
			"attest"
		} else {
			"seal"
		};
		let mut args = vec![action, "audits/old/v1.md", "--yes"];
		if lifecycle == "living" {
			args.push("--reread");
		}
		repo.run(&args).expect_code(0);
		repo.commit_at("2026-10-01T12:00:00Z");
		let before = repo.run(&["check", "--at", AT, "--format", "json"]).json();
		let series = |report: &Value| {
			report["subjects"]
				.as_array()
				.unwrap()
				.iter()
				.find(|s| s["kind"] == "series")
				.unwrap()
				.clone()
		};
		let before = series(&before);
		std::fs::create_dir_all(repo.path().join("audits/new")).unwrap();
		repo.git(&["mv", "audits/old/v1.md", "audits/new/v1.md"]);
		repo
			.run(&["rename", "audits/old/v1.md", "audits/new/v1.md", "--yes"])
			.expect_code(0);
		for committed in [false, true] {
			if committed {
				repo.commit_at("2026-11-03T12:00:00Z");
			}
			let after =
				series(&repo.run(&["check", "--at", AT, "--format", "json"]).json());
			assert_eq!(after["facts"], before["facts"]);
			assert_eq!(after["missed"], before["missed"]);
			let explained = repo
				.run(&["explain", "audits", "--at", AT])
				.expect_code(0)
				.stdout
				.clone();
			assert!(explained.contains("2026-10-01"), "{explained}");
		}
	}
}

/// Rooted provenance and rebased source obligations survive project changes.
#[test]
fn rename_history_reexpresses_existing_provenance_across_projects() {
	let repo = TestRepo::init();
	for project in ["one", "two"] {
		repo.write(
			&format!("{project}/.source-sentry/source-sentry.yaml"),
			"version: 1\n",
		);
	}
	repo.write("one/old.md", "# Guide\n");
	repo
		.run(&[
			"track",
			"one/old.md",
			"--source",
			":/src/a.rs",
			"--grace",
			"30",
			"--yes",
		])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");
	for (old, new) in [
		("one/old.md", "one/middle.md"),
		("one/middle.md", "two/new.md"),
	] {
		repo.git(&["mv", old, new]);
		let plan = repo
			.run(&["rename", old, new, "--yes"])
			.expect_code(0)
			.stdout
			.clone();
		if new == "two/new.md" {
			assert!(
				plan.contains("provenance `/old.md` -> `:/one/old.md`"),
				"{plan}"
			);
		}
		repo.commit_at("2026-02-01T12:00:00Z");
	}
	let lock: Value =
		serde_json::from_str(&repo.read("two/source-sentry.lock")).unwrap();
	assert_eq!(
		lock["documents"]["new.md"]["renamed"][0]["from"],
		":/one/old.md"
	);
	assert_eq!(
		lock["documents"]["new.md"]["renamed"][1]["from"],
		":/one/middle.md"
	);
	let report = repo
		.run(&["-C", "two", "check", "--at", AT, "--format", "json"])
		.json();
	assert_eq!(
		document(&report, "new.md")["unattested"]["since"],
		"2026-01-02T12:00:00Z"
	);
}

/// Project-rooted provenance resolves after an export removes its enclosing
/// directory from every historical tree.
#[test]
fn rename_history_resolves_in_an_exported_layout() {
	let mut exported_lockfile: Option<String> = None;
	for prefix in ["product/", ""] {
		let repo = TestRepo::init();
		repo.write(
			&format!("{prefix}.source-sentry/source-sentry.yaml"),
			"version: 1\n",
		);
		repo.write(&format!("{prefix}docs/old.md"), "# Guide\n");
		repo
			.run(&[
				"track",
				&format!("{prefix}docs/old.md"),
				"--grace",
				"30",
				"--yes",
			])
			.expect_code(0);
		repo.commit_at("2026-01-02T12:00:00Z");
		repo.git(&[
			"mv",
			&format!("{prefix}docs/old.md"),
			&format!("{prefix}docs/new.md"),
		]);
		repo
			.run(&[
				"rename",
				&format!("{prefix}docs/old.md"),
				&format!("{prefix}docs/new.md"),
				"--yes",
			])
			.expect_code(0);
		let lockfile = format!("{prefix}docs/source-sentry.lock");
		if let Some(bytes) = &exported_lockfile {
			repo.write(&lockfile, bytes);
		} else {
			exported_lockfile = Some(repo.read(&lockfile));
		}
		repo.commit_at("2026-02-01T12:00:00Z");
		let lock: Value = serde_json::from_str(
			&repo.read(&format!("{prefix}docs/source-sentry.lock")),
		)
		.unwrap();
		assert_eq!(
			lock["documents"]["new.md"]["renamed"][0]["from"],
			"/docs/old.md"
		);
		let directory = if prefix.is_empty() { "." } else { "product" };
		let report = repo
			.run(&["-C", directory, "check", "--at", AT, "--format", "json"])
			.json();
		assert_eq!(
			document(&report, "docs/new.md")["facts"],
			json!(["overdue", "unattested"])
		);
		assert_eq!(
			document(&report, "docs/new.md")["unattested"]["since"],
			"2026-01-02T12:00:00Z"
		);
	}
}

/// A repair after adding a project marker re-expresses every former path.
#[test]
fn rename_history_reexpresses_provenance_in_a_stranded_declaration_repair() {
	let repo = TestRepo::init();
	repo.write("product/old.md", "# Guide\n");
	repo
		.run(&["track", "product/old.md", "--lockfile", ".", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-01-02T12:00:00Z");
	repo.git(&["mv", "product/old.md", "product/new.md"]);
	repo
		.run(&["rename", "product/old.md", "product/new.md", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-02-01T12:00:00Z");
	repo.write("product/.source-sentry/source-sentry.yaml", "version: 1\n");
	let run = repo.run(&[
		"track",
		"product/new.md",
		"--from",
		"lockfile:.",
		"--to",
		"lockfile:product",
		"--yes",
	]);
	run.expect_code(0);
	assert!(
		run
			.stdout
			.contains("provenance `/product/old.md` -> `/old.md`"),
		"{}",
		run.stdout
	);
	let lock: Value =
		serde_json::from_str(&repo.read("product/source-sentry.lock")).unwrap();
	assert_eq!(lock["documents"]["new.md"]["renamed"][0]["from"], "/old.md");
}

/// Re-expressing sources preserves their independent declaration clocks,
/// including a pattern's first matched member, in both historical modes.
#[test]
fn rename_history_keeps_unreviewed_path_and_pattern_clocks() {
	for on_file in [false, true] {
		let repo = TestRepo::init();
		let placement = if on_file { "--on-file" } else { "--lockfile" };
		repo
			.run(&["track", "docs/guide.md", placement, "--yes"])
			.expect_code(0);
		repo
			.run(&["attest", "docs/guide.md", "--reread", "--yes"])
			.expect_code(0);
		repo.commit_at("2026-01-01T12:00:00Z");
		repo
			.run(&[
				"track",
				"docs/guide.md",
				"--source",
				"src/a.rs",
				"--pattern",
				"src/*.rs",
				"--grace",
				"30",
				"--yes",
			])
			.expect_code(0);
		repo.commit_at("2026-01-02T12:00:00Z");
		let before = document(
			&repo.run(&["check", "--at", AT, "--format", "json"]).json(),
			"docs/guide.md",
		);
		std::fs::create_dir_all(repo.path().join("archive/deep")).unwrap();
		repo.git(&["mv", "docs/guide.md", "archive/deep/new.md"]);
		repo
			.run(&["rename", "docs/guide.md", "archive/deep/new.md", "--yes"])
			.expect_code(0);
		for committed in [false, true] {
			if committed {
				repo.commit_at("2026-02-01T12:00:00Z");
			}
			let after = document(
				&repo.run(&["check", "--at", AT, "--format", "json"]).json(),
				"archive/deep/new.md",
			);
			assert_eq!(after["sources"], before["sources"]);
			assert_eq!(after["deadline"], before["deadline"]);
		}
	}
}

/// An unreadable historical declaration may advance an obligation but never
/// carries a seal across the gap to an earlier completion.
#[test]
fn rename_history_treats_unreadable_declarations_conservatively() {
	for sealed in [false, true] {
		let repo = TestRepo::init();
		repo.write(".source-sentry/source-sentry.yaml", "version: 1\nseries:\n  audits:\n    pattern: 'docs/(?:guide|new)(?P<version>\\d+)\\.md'\n    version-kind: integer\n    lifecycle: records-only\n    schedule:\n      cadence:\n        dtstart: 2026-02-01\n        rrule: FREQ=MONTHLY\n");
		repo.write("docs/guide1.md", "# Guide\n");
		repo
			.run(&["track", "docs/guide1.md", "--grace", "30", "--yes"])
			.expect_code(0);
		if sealed {
			repo
				.run(&["seal", "docs/guide1.md", "--yes"])
				.expect_code(0);
		}
		let declaration = repo.read("docs/source-sentry.lock");
		repo.commit_at("2026-01-01T12:00:00Z");
		repo.write("docs/source-sentry.lock", "{ invalid\n");
		repo.commit_at("2026-01-02T12:00:00Z");
		repo.write("docs/source-sentry.lock", &declaration);
		repo.commit_at("2026-02-03T12:00:00Z");
		repo.git(&["mv", "docs/guide1.md", "docs/new2.md"]);
		repo
			.run(&["rename", "docs/guide1.md", "docs/new2.md", "--yes"])
			.expect_code(0);
		let explained = repo
			.run(&[
				"explain",
				if sealed { "audits" } else { "docs/new2.md" },
				"--at",
				AT,
			])
			.expect_code(0)
			.stdout
			.clone();
		let expected = if sealed { "2026-02-03" } else { "2026-01-01" };
		assert!(explained.contains(expected), "{explained}");
	}
}
