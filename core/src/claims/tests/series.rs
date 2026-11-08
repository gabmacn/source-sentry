//! Series membership and versions.

use std::cmp::Ordering;

use super::{
	BASE, REPOSITORY, assert_clean, inventory, local, lockfile, only, tombstone,
};
use crate::claims::{
	DiagnosticKind, Inventory, MemberState, SeriesMembers, Severity, Version,
};
use crate::config::Scope;
use crate::model::VersionKind;
use crate::testing::path;

/// A repository config defining one series.
fn config(name: &str, pattern: &str, kind: &str) -> String {
	format!(
		"{BASE}series:\n  {name}:\n    pattern: '{pattern}'\n    version-kind: \
		 {kind}\n    lifecycle: records-only\n"
	)
}

/// The series named `name`.
fn series<'a>(inventory: &'a Inventory, name: &str) -> &'a SeriesMembers {
	inventory
		.series
		.iter()
		.find(|series| series.series.name == name)
		.unwrap()
}

/// A series' members as `(path, canonical version)`, in order.
fn members(series: &SeriesMembers) -> Vec<(String, String)> {
	series
		.members
		.iter()
		.map(|member| (member.path.to_string(), member.version.to_string()))
		.collect()
}

/// Pairs of strings, for comparisons.
fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
	expected
		.iter()
		.map(|(left, right)| ((*left).to_owned(), (*right).to_owned()))
		.collect()
}

#[test]
fn integer_versions_order_by_value() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("logs", r"logs/log-(?P<version>\d+)\.md", "integer"),
		),
		("logs/log-10.md", ""),
		("logs/log-2.md", ""),
		("logs/log-001.md", ""),
		("logs/notes.md", ""),
	]);
	assert_clean(&inventory);
	let logs = series(&inventory, "logs");
	assert_eq!(logs.series.scope, Scope::Repository);
	assert_eq!(
		members(logs),
		pairs(&[
			("logs/log-001.md", "1"),
			("logs/log-2.md", "2"),
			("logs/log-10.md", "10")
		])
	);
	// Nothing claims them, and they are members all the same.
	assert!(logs.members.iter().all(|member| !member.claimed));
	assert!(
		logs
			.members
			.iter()
			.all(|member| member.state == MemberState::Present)
	);
}

#[test]
fn dotted_versions_compare_by_component_with_trailing_zeros_ignored() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("proposals", r"v(?P<version>[0-9.]+)\.pdf", "dotted"),
		),
		("v0.0.9.pdf", ""),
		("v0.0.10.pdf", ""),
		("v0.3.pdf", ""),
		("v1.pdf", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		members(series(&inventory, "proposals")),
		pairs(&[
			("v0.0.9.pdf", "0.0.9"),
			("v0.0.10.pdf", "0.0.10"),
			("v0.3.pdf", "0.3"),
			("v1.pdf", "1")
		])
	);
}

#[test]
fn date_versions_order_by_date() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("daily", r"logs/(?P<version>[0-9-]+)\.md", "date"),
		),
		("logs/2026-01-05.md", ""),
		("logs/2025-12-31.md", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		members(series(&inventory, "daily")),
		pairs(&[
			("logs/2025-12-31.md", "2025-12-31"),
			("logs/2026-01-05.md", "2026-01-05")
		])
	);
}

#[test]
fn equal_versions_are_a_configuration_error() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("proposals", r"v(?P<version>[0-9.]+)\.pdf", "dotted"),
		),
		("v0.3.pdf", ""),
		("v0.3.0.pdf", ""),
		("v0.4.pdf", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::DuplicateVersion);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(diagnostic.location.file, path("v0.3.0.pdf"));
	let related: Vec<_> = diagnostic
		.related
		.iter()
		.map(|location| location.file.to_string())
		.collect();
	assert_eq!(related, [REPOSITORY, "v0.3.pdf"]);
	assert!(diagnostic.message.contains("version 0.3"));
	// Both are kept, so evaluation can see the conflict.
	assert_eq!(series(&inventory, "proposals").members.len(), 3);
}

#[test]
fn a_file_in_two_series_is_a_configuration_error() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("all", r"logs/(?P<version>\d+)\.md", "integer"),
		),
		(
			"logs/source-sentry.yaml",
			"series:\n  short:\n    pattern: '(?P<version>\\d)\\.md'\n    \
			 version-kind: integer\n    lifecycle: living\n",
		),
		("logs/1.md", ""),
		("logs/12.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::SeriesOverlap);
	assert_eq!(diagnostic.location.file, path("logs/1.md"));
	assert_eq!(diagnostic.related.len(), 2);
	assert_eq!(diagnostic.document, Some(local("logs/1.md")));
	let short = series(&inventory, "short");
	assert_eq!(short.series.scope, Scope::Directory(path("logs")));
	assert_eq!(short.members, []);
	assert_eq!(
		members(series(&inventory, "all")),
		pairs(&[("logs/12.md", "12")])
	);
}

#[test]
fn a_version_that_does_not_parse_is_a_configuration_error() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("daily", r"(?P<version>[0-9-]+)\.md", "date"),
		),
		("2026-02-30.md", ""),
		("2026-2-3.md", ""),
		("2026-02-03.md", ""),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::InvalidVersion);
			assert_eq!(diagnostic.related[0].file, path(REPOSITORY));
			// The config module positions a series at its definition's body.
			assert_eq!(diagnostic.related[0].line, Some(8));
			assert_eq!(diagnostic.related[0].field.as_deref(), Some("series.daily"));
			diagnostic.location.file.to_string()
		})
		.collect();
	assert_eq!(found, ["2026-02-30.md", "2026-2-3.md"]);
	assert_eq!(
		members(series(&inventory, "daily")),
		pairs(&[("2026-02-03.md", "2026-02-03")])
	);
}

#[test]
fn listed_and_retired_entries_stay_members() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&config("audits", r"audits/audit-(?P<version>\d+)\.md", "integer"),
		),
		(
			"audits/source-sentry.lock",
			&lockfile(
				&[("audit-0002.md", "{}"), ("audit-0003.md", "{}")],
				&[("audit-0001.md", &tombstone())],
			),
		),
		("audits/audit-0002.md", ""),
		("audits/audit-0004.md", ""),
	]);
	let audits = series(&inventory, "audits");
	let states: Vec<_> = audits
		.members
		.iter()
		.map(|member| (member.path.to_string(), member.state, member.claimed))
		.collect();
	assert_eq!(
		states,
		[
			(
				"audits/audit-0001.md".to_owned(),
				MemberState::Retired,
				false
			),
			(
				"audits/audit-0002.md".to_owned(),
				MemberState::Present,
				true
			),
			(
				"audits/audit-0003.md".to_owned(),
				MemberState::Missing,
				true
			),
			(
				"audits/audit-0004.md".to_owned(),
				MemberState::Present,
				false
			),
		]
	);
	let (found, member) =
		inventory.series_of(&path("audits/audit-0004.md")).unwrap();
	assert_eq!(found.series.name, "audits");
	assert_eq!(member.version.to_string(), "4");
}

#[test]
fn a_series_without_members_is_listed() {
	let inventory = inventory(&[(
		REPOSITORY,
		&config("audits", r"audit-(?P<version>\d+)\.md", "integer"),
	)]);
	assert_clean(&inventory);
	assert_eq!(inventory.series.len(), 1);
	assert_eq!(inventory.series[0].members, []);
}

#[test]
fn versions_parse_by_kind() {
	let parse = |kind, text| Version::parse(kind, text);
	let order = |kind, left, right| {
		parse(kind, left).unwrap().cmp(&parse(kind, right).unwrap())
	};
	assert_eq!(order(VersionKind::Integer, "01", "1"), Ordering::Equal);
	assert_eq!(order(VersionKind::Integer, "9", "10"), Ordering::Less);
	assert_eq!(
		order(VersionKind::Integer, "100000000000000000000000000000", "99"),
		Ordering::Greater
	);
	assert_eq!(order(VersionKind::Dotted, "0.3", "0.3.0"), Ordering::Equal);
	assert_eq!(order(VersionKind::Dotted, "0", "0.0"), Ordering::Equal);
	assert_eq!(order(VersionKind::Dotted, "0.3", "0.3.1"), Ordering::Less);
	assert_eq!(order(VersionKind::Dotted, "1.10", "1.9"), Ordering::Greater);
	assert_eq!(
		order(VersionKind::Date, "2025-12-31", "2026-01-01"),
		Ordering::Less
	);
	assert_eq!(parse(VersionKind::Dotted, "0.0").unwrap().to_string(), "0");
	assert_eq!(
		parse(VersionKind::Dotted, "01.20.0").unwrap().to_string(),
		"1.20"
	);
	for (kind, text) in [
		(VersionKind::Integer, ""),
		(VersionKind::Integer, "+1"),
		(VersionKind::Integer, "1.0"),
		(VersionKind::Dotted, "1..2"),
		(VersionKind::Dotted, ".1"),
		(VersionKind::Dotted, "v1"),
		(VersionKind::Date, "2026-02-30"),
		(VersionKind::Date, "+2026-02-03"),
		(VersionKind::Date, "20260203"),
	] {
		let error = parse(kind, text).unwrap_err();
		assert_eq!(error.kind, kind);
		assert_eq!(error.text, text);
	}
}
