//! Field locations in local and remote lockfiles, including tombstones.

use super::{BASE, REPOSITORY, REVIEW, inventory, lockfile, only};
use crate::claims::{DiagnosticKind, entry_field};
use crate::testing::path;

/// Local and remote lockfiles locate a pattern's review separately from members.
#[test]
fn lockfiles_locate_pattern_reviews() {
	for (key, file) in [
		("guide.md", "source-sentry.lock"),
		("abstract:guide", ".source-sentry/remotes/team.lock"),
	] {
		let lock = format!(
			r#"{{
  "documents": {{{key:?}: {{"sources": [{{
    "pattern": "/src/*.rs",
    "reviewed": {{
      "at": "2026-11-07T14:32:05Z",
      "by": null
    }}
  }}]}}}},
  "retired": {{}},
  "schema": 1
}}"#,
		);
		let inventory = inventory(&[(REPOSITORY, BASE), (file, &lock)]);
		assert!(inventory.lockfiles[0].parsed);
		let claim = &inventory.claims[0];
		for (suffix, line) in
			[("reviewed", 4), ("reviewed.at", 5), ("reviewed.by", 6)]
		{
			assert_eq!(
				claim.lines[&format!("{}.sources[0].{suffix}", claim.field)],
				line
			);
		}
	}
}

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
    {quoted}: [{{
      "retired": {REVIEW}
    }},
    {{
      "retired": {REVIEW}
    }}],
    "{neighbor}": [{{
      "retired": {REVIEW}
    }}]
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

		for (index, line) in [(0, 17), (1, 20)] {
			let field = format!("{}[{index}]", entry_field("retired", key));
			let tombstone = inventory
				.tombstones
				.iter()
				.find(|tombstone| tombstone.field == field)
				.unwrap();
			let location = tombstone.location_of(field.clone());
			assert_eq!(location.line, Some(line));
			let location = tombstone.location_of(format!("{field}.retired.at"));
			assert_eq!(location.file, path(file));
			assert_eq!(location.line, Some(line + 1));
			assert_eq!(location.column, None);
			assert_eq!(tombstone.lines.len(), 4, "{:?}", tombstone.lines);
			assert_eq!(tombstone.location_of(format!("{field}.seal")).line, None);
		}
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
    "nested/old.md": [{{
      "retired": {REVIEW}
    }}]
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
		Some(r#"retired["nested/old.md"][0]"#)
	);
}

#[test]
fn a_shared_remote_tombstone_has_lines_in_both_owner_lockfiles() {
	let lock = format!(
		r#"{{
  "documents": {{}},
  "retired": {{
    "abstract:old": [{{
      "retired": {REVIEW}
    }}]
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
		Some(r#"retired["abstract:old"][0]"#)
	);
}

/// Every tombstone outside the nearest lockfile has its own indexed field
/// and item line, even when another tombstone shares its path.
#[test]
fn authority_checks_every_tombstone_in_a_list() {
	let lock = format!(
		r#"{{
  "documents": {{}},
  "retired": {{
    "nested/old.md": [
      {{"retired": {REVIEW}}},
      {{"retired": {REVIEW}}}
    ]
  }},
  "schema": 1
}}"#
	);
	let inventory = inventory(&[
		("docs/source-sentry.lock", &lock),
		("docs/nested/source-sentry.lock", &lockfile(&[], &[])),
	]);
	assert_eq!(inventory.diagnostics.len(), 2);
	for (index, diagnostic) in inventory.diagnostics.iter().enumerate() {
		assert_eq!(diagnostic.kind, DiagnosticKind::OutsideAuthority);
		assert_eq!(diagnostic.location.line, Some(5 + index as u64));
		assert_eq!(
			diagnostic.location.field,
			Some(format!("retired[\"nested/old.md\"][{index}]"))
		);
	}
}

/// Empty and out-of-order lists are configuration errors with JSON lines
/// in local and remote lockfiles alike.
#[test]
fn invalid_tombstone_lists_report_lines_in_diagnostics() {
	for (file, key) in [
		("docs/source-sentry.lock", "old.md"),
		(".source-sentry/remotes/legal.lock", "abstract:old"),
	] {
		for value in [
			"[]".to_owned(),
			format!(
				"[{{\"retired\": {REVIEW}}}, {{\"retired\": {{\"at\": \"2026-01-01T00:00:00Z\", \"by\": null}}}}]"
			),
		] {
			let lock = format!(
				"{{\n\"documents\": {{}},\n\"retired\": {{{key:?}: {value}}},\n\"schema\": 1\n}}"
			);
			let inventory = inventory(&[(file, &lock)]);
			let diagnostic = only(&inventory, DiagnosticKind::InvalidLockfile);
			assert_eq!(diagnostic.location.line, Some(3));
			assert_eq!(diagnostic.location.field, Some(entry_field("retired", key)));
			assert_eq!(inventory.tombstones, []);
		}
	}
}

/// A later tombstone's seal and completion retain their own field lines.
#[test]
fn each_tombstone_keeps_its_marker_lines() {
	let lock = format!(
		r#"{{
  "documents": {{}},
  "retired": {{
    "old.md": [
      {{
        "completed": {REVIEW},
        "retired": {REVIEW},
        "seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {}}}
      }},
      {{
        "completed": {REVIEW},
        "retired": {REVIEW},
        "seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {}}}
      }}
    ]
  }},
  "schema": 1
}}"#,
		super::OID,
		super::OID
	);
	let inventory = inventory(&[("docs/source-sentry.lock", &lock)]);
	assert_eq!(inventory.diagnostics, []);
	assert_eq!(inventory.tombstones.len(), 2);
	for (index, tombstone) in inventory.tombstones.iter().enumerate() {
		for (marker, line) in [("completed", 6), ("seal", 8), ("seal.oid", 8)] {
			let location =
				tombstone.location_of(format!("{}.{marker}", tombstone.field));
			assert_eq!(location.line, Some(line + 5 * index as u64));
		}
		assert!(
			tombstone
				.lines
				.keys()
				.all(|field| field.starts_with(&tombstone.field))
		);
	}
}
