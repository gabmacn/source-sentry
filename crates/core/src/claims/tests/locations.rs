//! Field locations in local and remote lockfiles, including tombstones.

use super::{BASE, REPOSITORY, REVIEW, inventory, lockfile, only};
use crate::claims::{DiagnosticKind, entry_field};
use crate::testing::path;

/// Discovery checks schedules even when a sealed record needs no deadline.
#[test]
fn a_sealed_records_cadence_is_validated_during_discovery() {
	let lock = r#"{
  "documents": {
    "guide.md": {
      "schedule": {
        "cadence": {
          "dtstart": "2026-01-02",
          "rrule": "FREQ=MONTHLY;BYMONTHDAY=1"
        }
      },
      "seal": {
        "at": "2026-10-01T10:00:00Z",
        "by": null,
        "oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709"
      }
    }
  },
  "retired": {},
  "schema": 1
}"#;
	let inventory = inventory(&[
		("docs/source-sentry.lock", lock),
		("docs/guide.md", "# Guide\n"),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::InvalidSchedule);
	assert_eq!(diagnostic.location.file, path("docs/source-sentry.lock"));
	assert_eq!(diagnostic.location.line, Some(5));
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("documents[\"guide.md\"].schedule.cadence")
	);
	assert!(inventory.has_errors());
}

#[test]
fn lockfile_fields_keep_their_entry_keys_and_lines() {
	for (key, remote) in [
		("guide.v2.md", false),
		("nested/guide.md", false),
		("a\"quoted.md", false),
		("é/指南😀.md", false),
		("a\"].classes[0].md", false),
		("abstract:guide.v2", true),
		("google-drive:A_12-b.3", true),
	] {
		let neighbor = if remote {
			"abstract:neighbor"
		} else {
			"neighbor.md"
		};
		let quoted = serde_json::to_string(key).unwrap();
		let lock = format!(
			r#"{{
  "documents": {{
    {quoted}: {{
      "classes": [
        "internal-docs",
        "missing"
      ],
      "sources": [
        {{
          "path": "/absent.rs"
        }}
      ]
    }},
    "{neighbor}": {{}}
  }},
  "retired": {{
    {quoted}: {{
      "retired": {REVIEW}
    }},
    "{neighbor}": {{
      "retired": {REVIEW}
    }}
  }},
  "schema": 1
}}"#
		);
		let file = if remote {
			".source-sentry/remotes/team.lock"
		} else {
			"docs/source-sentry.lock"
		};
		// No real filesystem paths are needed, so quotes work on Windows too.
		let inventory = inventory(&[(REPOSITORY, BASE), (file, &lock)]);
		assert!(inventory.lockfiles[0].parsed, "{key}: {inventory:#?}");
		let field = entry_field("documents", key);
		let claim = inventory
			.claims
			.iter()
			.find(|claim| claim.field == field)
			.unwrap();
		assert_eq!(claim.lines[&field], 3);
		for (suffix, line) in [
			("classes", 4),
			("classes[1]", 6),
			("sources[0]", 9),
			("sources[0].path", 10),
		] {
			let location = claim.location_of(format!("{field}.{suffix}"));
			assert_eq!(location.file, path(file));
			assert_eq!(location.line, Some(line), "{location:?}");
			assert_eq!(location.column, None);
		}
		assert_eq!(claim.lines.len(), 7, "{:?}", claim.lines);
		let absent = claim.location_of(format!("{field}.reviewed"));
		assert_eq!(absent.line, None);
		assert_eq!(absent.field, Some(format!("{field}.reviewed")));

		let field = entry_field("retired", key);
		let tombstone = inventory
			.tombstones
			.iter()
			.find(|tombstone| tombstone.field == field)
			.unwrap();
		let location = tombstone.location_of(field.clone());
		assert_eq!(location.line, Some(17));
		let location = tombstone.location_of(format!("{field}.retired.at"));
		assert_eq!(location.file, path(file));
		assert_eq!(location.line, Some(18));
		assert_eq!(location.column, None);
		assert_eq!(tombstone.lines.len(), 4, "{:?}", tombstone.lines);
		assert_eq!(tombstone.location_of(format!("{field}.seal")).line, None);
	}
}

#[test]
fn escaped_json_keys_use_the_decoded_diagnostic_spelling() {
	let inventory = inventory(&[(
		"docs/source-sentry.lock",
		r#"{
  "documents": {
    "\u00e9\/a\u002e\uD83D\uDE00md": {
      "classes": ["missing"]
    }
  },
  "retired": {},
  "schema": 1
}"#,
	)]);
	assert!(inventory.lockfiles[0].parsed);
	let diagnostic = inventory
		.diagnostics
		.iter()
		.find(|diagnostic| diagnostic.kind == DiagnosticKind::UndeclaredClass)
		.unwrap();
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("documents[\"é/a.😀md\"].classes[0]")
	);
	assert_eq!(diagnostic.location.line, Some(4));
}

#[test]
fn a_tombstone_outside_its_lockfiles_authority_has_its_entry_line() {
	let lock = format!(
		r#"{{
  "documents": {{}},
  "retired": {{
    "nested/old.md": {{
      "retired": {REVIEW}
    }}
  }},
  "schema": 1
}}"#
	);
	let inventory = inventory(&[
		("docs/source-sentry.lock", &lock),
		("docs/nested/source-sentry.lock", &lockfile(&[], &[])),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::OutsideAuthority);
	assert_eq!(diagnostic.location.line, Some(4));
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some(r#"retired["nested/old.md"]"#)
	);
}

#[test]
fn a_shared_remote_tombstone_has_lines_in_both_owner_lockfiles() {
	let lock = format!(
		r#"{{
  "documents": {{}},
  "retired": {{
    "abstract:old": {{
      "retired": {REVIEW}
    }}
  }},
  "schema": 1
}}"#
	);
	let other = format!("\n\n{lock}");
	let inventory = inventory(&[
		(".source-sentry/remotes/a.lock", &lock),
		(".source-sentry/remotes/b.lock", &other),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::RetiredInAnotherLockfile);
	assert_eq!(diagnostic.location.line, Some(4));
	assert_eq!(diagnostic.related[0].line, Some(6));
	assert_eq!(
		diagnostic.related[0].field.as_deref(),
		Some(r#"retired["abstract:old"]"#)
	);
}
