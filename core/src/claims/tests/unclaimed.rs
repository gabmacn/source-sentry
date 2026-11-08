//! Unclaimed files: roots, nested roots, ignores and claims anywhere.

use super::{
	BASE, REPOSITORY, assert_clean, inventory, lockfile, markdown, paths,
};
use crate::claims::Inventory;
use crate::testing::path;

/// Every unclaimed file with its root, as strings.
fn unclaimed(inventory: &Inventory) -> Vec<(String, String)> {
	inventory
		.unclaimed
		.iter()
		.map(|file| (file.path.to_string(), file.root.to_string()))
		.collect()
}

/// An empty lockfile.
fn empty() -> String {
	lockfile(&[], &[])
}

#[test]
fn a_scan_stops_where_a_nested_root_begins() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("tracked.md", "{}")], &[]),
		),
		("docs/tracked.md", ""),
		("docs/new.md", ""),
		("docs/guide/new.txt", ""),
		("docs/policy.pdf", ""),
		("docs/main.rs", ""),
		("docs/api/source-sentry.lock", &empty()),
		("docs/api/auth.md", ""),
		("notes/outside.md", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		unclaimed(&inventory),
		[
			("docs/api/auth.md".to_owned(), "docs/api".to_owned()),
			("docs/guide/new.txt".to_owned(), "docs".to_owned()),
			("docs/new.md".to_owned(), "docs".to_owned()),
			("docs/policy.pdf".to_owned(), "docs".to_owned()),
		]
	);
}

#[test]
fn ignored_files_are_not_unclaimed() {
	let inventory = inventory(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"documents:\n  ignore:\n    - 'scratch/'\n    - '!scratch/keep.md'\n",
		),
		("docs/source-sentry.lock", &empty()),
		("docs/scratch/draft.md", ""),
		("docs/scratch/keep.md", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		unclaimed(&inventory),
		[("docs/scratch/keep.md".to_owned(), "docs".to_owned())]
	);
}

#[test]
fn root_false_lets_the_outer_scan_continue() {
	let inventory = inventory(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.lock", &empty()),
		("docs/sub/source-sentry.yaml", "root: false\n"),
		("docs/sub/source-sentry.lock", &empty()),
		("docs/sub/page.md", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		unclaimed(&inventory),
		[("docs/sub/page.md".to_owned(), "docs".to_owned())]
	);
	// The lockfile keeps its authority, whatever `root` says.
	assert_eq!(
		inventory
			.lockfile_for(&path("docs/sub/page.md"))
			.map(|lockfile| lockfile.path.to_string()),
		Some("docs/sub/source-sentry.lock".to_owned())
	);
}

#[test]
fn a_config_alone_makes_a_root_only_with_root_true() {
	let alone = inventory(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.yaml", "owners: ['platform']\n"),
		("docs/page.md", ""),
	]);
	assert_eq!(alone.unclaimed, []);

	let explicit = inventory(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.yaml", "root: true\n"),
		("docs/page.md", ""),
	]);
	assert_eq!(
		unclaimed(&explicit),
		[("docs/page.md".to_owned(), "docs".to_owned())]
	);
}

#[test]
fn a_claim_from_anywhere_counts_and_own_files_are_never_unclaimed() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&format!("{BASE}documents:\n  include: ['*.md', '*.yaml', '*.lock']\n"),
		),
		("source-sentry.lock", &empty()),
		("docs/on-file.md", &markdown("  owners: ['platform']\n")),
		("docs/source-sentry.yaml", "owners: ['platform']\n"),
		(".source-sentry/presets/README.md", ""),
		("docs/left.md", ""),
	]);
	assert_clean(&inventory);
	let found: Vec<_> = inventory
		.unclaimed
		.iter()
		.map(|file| file.path.clone())
		.collect();
	assert_eq!(found, paths(&["docs/left.md"]));
}
