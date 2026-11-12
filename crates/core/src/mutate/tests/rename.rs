//! Document renames preserve reviews while changing their path identities.

use super::{
	PLAIN, REVIEW, SOURCE, Tree, at, attested, local, lockfile, markdown, oid,
	path, remote, seal_of, series_config,
};
use crate::adapters::frontmatter;
use crate::claims::DiagnosticKind;
use crate::model::SourceDecl;
use crate::mutate::{
	MutateError, Planned, RenameRequest, RenameSourceKind, RenameSummary, rename,
};
use crate::ports::Snapshot as _;
use crate::testing::{FakeHistory, FakeSnapshot};

/// Plans a lockfile rename without needing any committed history.
fn plan(
	tree: &Tree,
	old: &str,
	new: &str,
) -> Result<Planned<RenameSummary>, MutateError> {
	rename(
		&tree.snapshot,
		&FakeHistory::new(),
		&tree.configs,
		&tree.inventory,
		&RenameRequest {
			old: local(old),
			new: local(new),
		},
	)
}

/// A missing lockfile document and its already moved destination.
fn moved(declaration: &str, extra: &[(&str, &str)]) -> Tree {
	let lock = lockfile(&[("guide.md", declaration)], &[]);
	let mut files = vec![
		("docs/source-sentry.lock", lock.as_str()),
		("archive/deep/guide.md", PLAIN),
		("src/a.rs", SOURCE),
	];
	files.extend_from_slice(extra);
	Tree::new(&files)
}

/// Stored reviews, a seal and completion survive an authority change; all
/// relative spellings, including pattern members, retain their targets.
#[test]
fn a_sealed_lockfile_document_keeps_every_marker_and_rebases_sources() {
	let declaration = format!(
		r#"{{"reviewed": {REVIEW}, "completed": {REVIEW}, "seal": {}, "sources": [{}, {{"pattern": "../src/*.rs", "members": [{}]}}]}}"#,
		seal_of(PLAIN),
		attested("../src/a.rs", SOURCE),
		attested("/src/a.rs", SOURCE)
	);
	let tree = moved(&declaration, &[]);
	let before = tree.claim(&local("docs/guide.md")).declaration.clone();
	let planned = plan(&tree, "docs/guide.md", "archive/deep/guide.md").unwrap();
	assert_eq!(planned.summary.from.file, path("docs/source-sentry.lock"));
	assert_eq!(
		planned.summary.to.file,
		path("archive/deep/source-sentry.lock")
	);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert!(
		!after
			.snapshot
			.paths()
			.unwrap()
			.contains(&path("docs/source-sentry.lock"))
	);
	let renamed = &after.claim(&local("archive/deep/guide.md")).declaration;
	assert_eq!(renamed.reviewed, before.reviewed);
	assert_eq!(renamed.seal, before.seal);
	assert_eq!(renamed.completed, before.completed);
	let mut expected = before;
	if let SourceDecl::Path(source) = &mut expected.sources[0] {
		source.path = "../../src/a.rs".to_owned();
	}
	if let SourceDecl::Pattern(source) = &mut expected.sources[1] {
		source.pattern = "../../src/*.rs".to_owned();
	}
	expected
		.sources
		.sort_by(|left, right| left.declared().cmp(right.declared()));
	assert_eq!(*renamed, expected);
}

/// A destination's existing nearest lockfile receives the live declaration
/// while every retirement under the old path stays in its original file.
#[test]
fn destination_authority_receives_the_entry_and_old_tombstones_stay() {
	let tombstone = format!(r#"{{"retired": {REVIEW}}}"#);
	let old = lockfile(&[("guide.md", "{}")], &[("guide.md", &tombstone)]);
	let destination = lockfile(&[("other.md", "{}")], &[]);
	let tree = Tree::new(&[
		("docs/source-sentry.lock", &old),
		("archive/source-sentry.lock", &destination),
		("archive/other.md", PLAIN),
		("archive/deep/guide.md", PLAIN),
	]);
	let planned = plan(&tree, "docs/guide.md", "archive/deep/guide.md").unwrap();
	assert_eq!(planned.summary.to.file, path("archive/source-sentry.lock"));
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(after.inventory.tombstones.len(), 1);
	assert_eq!(
		after.inventory.tombstones[0].retired,
		tree.inventory.tombstones[0].retired
	);
	assert_eq!(
		after.inventory.tombstones[0].lockfile,
		path("docs/source-sentry.lock")
	);
	assert!(
		after
			.inventory
			.claims_of(&local("docs/guide.md"))
			.next()
			.is_none()
	);
	assert!(after.text("docs/source-sentry.lock").contains("retired"));
}

/// A rename can remain within the same ancestor lockfile.
#[test]
fn a_root_lockfile_keeps_authority_over_the_destination() {
	let tree = Tree::new(&[
		(
			"source-sentry.lock",
			&lockfile(&[("docs/guide.md", "{}")], &[]),
		),
		("archive/deep/guide.md", PLAIN),
	]);
	let planned = plan(&tree, "docs/guide.md", "archive/deep/guide.md").unwrap();
	assert_eq!(planned.summary.to.file, path("source-sentry.lock"));
	tree.apply(&planned.plan).assert_clean();
}

/// On-file provenance comes from HEAD; the arrived declaration is rebased
/// without changing other frontmatter keys, line endings or body bytes.
#[test]
fn an_on_file_declaration_travels_and_keeps_other_bytes() {
	let text = markdown(&format!("  reviewed:\n    at: 2026-11-01T00:00:00Z\n    by: Bruno Costa\n  sources:\n    - path: ../src/a.rs\n      oid: '{}'\n      reviewed:\n        at: 2026-11-01T00:00:00Z\n        by: Bruno Costa\n    - pattern: ../src/*.rs\n      members:\n        - path: ../src/a.rs\n          oid: '{}'\n          reviewed:\n            at: 2026-11-01T00:00:00Z\n            by: Bruno Costa\n", oid(SOURCE), oid(SOURCE))).replace('\n', "\r\n");
	let mut history = FakeHistory::new();
	let head = history.commit(at(), &[("docs/guide.md", Some(text.as_bytes()))]);
	let tree = Tree::of(
		FakeSnapshot::working_tree(Some(head))
			.with_file("archive/deep/guide.md", text.as_bytes())
			.with_file("src/a.rs", SOURCE.as_bytes()),
	);
	let planned = rename(
		&tree.snapshot,
		&history,
		&tree.configs,
		&tree.inventory,
		&RenameRequest {
			old: local("docs/guide.md"),
			new: local("archive/deep/guide.md"),
		},
	)
	.unwrap();
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let result = after.text("archive/deep/guide.md");
	assert!(result.starts_with("---\r\ntitle: 'Guide'\r\n"));
	assert!(result.ends_with("---\r\n# Guide\r\n\r\nBody.\r\n"));
	let before = frontmatter::read(&path("docs/guide.md"), text.as_bytes())
		.unwrap()
		.unwrap();
	let declaration = &after.claim(&local("archive/deep/guide.md")).declaration;
	assert_eq!(declaration.reviewed, before.reviewed);
	assert_eq!(declaration.sources[0].declared(), "../../src/a.rs");
	let SourceDecl::Pattern(pattern) = &declaration.sources[1] else {
		panic!("pattern");
	};
	assert_eq!(pattern.members[0].path, "../../src/a.rs");
	assert_eq!(pattern.members[0].oid, oid(SOURCE));
}

/// On-file destinations must preserve a committed origin declaration and
/// must not already have an independent declaration at HEAD.
#[test]
fn ambiguous_on_file_origins_are_refused() {
	let original = markdown("  sources:\n    - path: /src/a.rs\n");
	let altered = markdown("  sources: []\n");
	for (committed, arrived, destination) in [
		(false, original.as_str(), false),
		(true, altered.as_str(), false),
		(true, original.as_str(), true),
	] {
		let mut history = FakeHistory::new();
		if committed {
			let mut files = vec![("docs/guide.md", Some(original.as_bytes()))];
			if destination {
				files.push(("archive/guide.md", Some(original.as_bytes())));
			}
			history.commit(at(), &files);
		}
		let tree = Tree::of(
			FakeSnapshot::working_tree(history.head())
				.with_file("archive/guide.md", arrived.as_bytes())
				.with_file("src/a.rs", SOURCE.as_bytes()),
		);
		let result = rename(
			&tree.snapshot,
			&history,
			&tree.configs,
			&tree.inventory,
			&RenameRequest {
				old: local("docs/guide.md"),
				new: local("archive/guide.md"),
			},
		);
		assert!(
			matches!(result, Err(MutateError::RenameRefused { .. })),
			"{result:?}"
		);
	}
}

/// An on-file declaration may move only to another declaration-capable
/// file; otherwise it would silently stop being discovered.
#[test]
fn an_on_file_destination_requires_a_declaration_adapter() {
	let original = markdown("  sources: []\n");
	let mut history = FakeHistory::new();
	history.commit(at(), &[("docs/guide.md", Some(original.as_bytes()))]);
	let tree = Tree::of(
		FakeSnapshot::working_tree(history.head())
			.with_file("guide.pdf", original.as_bytes()),
	);
	let result = rename(
		&tree.snapshot,
		&history,
		&tree.configs,
		&tree.inventory,
		&RenameRequest {
			old: local("docs/guide.md"),
			new: local("guide.pdf"),
		},
	);
	assert!(
		matches!(result, Err(MutateError::NotDeclarationCapable { .. })),
		"{result:?}"
	);
}

/// Dependents may be declared in frontmatter or sealed lockfile entries;
/// updating their source metadata preserves every review and seal.
#[test]
fn on_file_and_sealed_dependents_follow_the_renamed_source() {
	let on_file = markdown(&format!(
		"  sources:\n    - path: docs/guide.md\n      oid: '{}'\n      reviewed:\n        at: 2026-11-01T00:00:00Z\n        by: Bruno Costa\n",
		oid(PLAIN)
	));
	let sealed = format!(
		r#"{{"seal": {}, "sources": [{}]}}"#,
		seal_of(PLAIN),
		attested("docs/guide.md", PLAIN)
	);
	let tree = moved(
		"{}",
		&[
			("reader.md", &on_file),
			("record.txt", PLAIN),
			(
				"source-sentry.lock",
				&lockfile(&[("record.txt", &sealed)], &[]),
			),
		],
	);
	let before = tree.claim(&local("record.txt")).declaration.seal.clone();
	let planned = plan(&tree, "docs/guide.md", "archive/deep/guide.md").unwrap();
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	for document in ["reader.md", "record.txt"] {
		let SourceDecl::Path(source) =
			&after.claim(&local(document)).declaration.sources[0]
		else {
			panic!("path");
		};
		assert_eq!(source.path, "archive/deep/guide.md");
		assert_eq!(source.oid, Some(oid(PLAIN)));
		assert!(source.reviewed.is_some());
	}
	assert_eq!(after.claim(&local("record.txt")).declaration.seal, before);
}

/// Editing the moved document does not invent reviews of its new content;
/// dependent baselines stay at the oid that was actually reviewed.
#[test]
fn edited_content_keeps_the_old_review_oid() {
	let dependent =
		format!(r#"{{"sources": [{}]}}"#, attested("/docs/guide.md", PLAIN));
	let tree = Tree::new(&[
		(
			"source-sentry.lock",
			&lockfile(&[("docs/guide.md", "{}"), ("reader.txt", &dependent)], &[]),
		),
		("reader.txt", PLAIN),
		("archive/guide.md", "Changed content\n"),
	]);
	let planned = plan(&tree, "docs/guide.md", "archive/guide.md").unwrap();
	let after = tree.apply(&planned.plan);
	let SourceDecl::Path(source) =
		&after.claim(&local("reader.txt")).declaration.sources[0]
	else {
		panic!("path");
	};
	assert_eq!(source.oid, Some(oid(PLAIN)));
}

/// Existing sources must not become self-dependencies at the new path.
#[test]
fn a_new_self_dependency_is_refused() {
	let tree = moved(r#"{"sources": [{"path": "/archive/deep/guide.md"}]}"#, &[]);
	let Err(MutateError::WouldIntroduce { diagnostics }) =
		plan(&tree, "docs/guide.md", "archive/deep/guide.md")
	else {
		panic!("self dependency must refuse");
	};
	assert!(
		diagnostics
			.iter()
			.any(|diagnostic| diagnostic.kind == DiagnosticKind::SelfDependency)
	);
}

/// Rooted dependent sources, relative dependent sources and remote sources
/// all follow the new path without losing their baseline or review.
#[test]
fn dependent_path_sources_keep_their_reviews() {
	let dependent = format!(
		r#"{{"reviewed": {REVIEW}, "sources": [{}]}}"#,
		attested("guide.md", PLAIN)
	);
	let remote_declaration =
		format!(r#"{{"sources": [{}]}}"#, attested("/docs/guide.md", PLAIN));
	let tree = Tree::new(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("guide.md", "{}"),
					("other.md", &dependent),
					("rooted.md", &remote_declaration),
				],
				&[],
			),
		),
		("docs/other.md", PLAIN),
		("docs/rooted.md", PLAIN),
		("archive/guide.md", PLAIN),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", &remote_declaration)], &[]),
		),
	]);
	let planned = plan(&tree, "docs/guide.md", "archive/guide.md").unwrap();
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let dependent = &after.claim(&local("docs/other.md")).declaration;
	assert_eq!(dependent.sources[0].declared(), "../archive/guide.md");
	assert_eq!(
		after.claim(&local("docs/rooted.md")).declaration.sources[0].declared(),
		"/archive/guide.md"
	);
	for source in &dependent.sources {
		let SourceDecl::Path(source) = source else {
			panic!("path");
		};
		assert_eq!(source.oid, Some(oid(PLAIN)));
		assert!(source.reviewed.is_some());
	}
	assert_eq!(
		after.claim(&remote("abstract:video")).declaration.sources[0].declared(),
		"/archive/guide.md"
	);
}

/// Reviewed members follow a rename only while the pattern still matches.
/// The unchanged pattern keeps the removed member for `check` to report.
#[test]
fn pattern_reviews_follow_matching_destinations_only() {
	let declaration = format!(
		r#"{{"sources": [{{"pattern": "/**/*.md", "members": [{}]}}, {{"pattern": "/docs/*.md", "members": [{}]}}]}}"#,
		attested("/docs/guide.md", PLAIN),
		attested("/docs/guide.md", PLAIN)
	);
	let tree = moved(
		"{}",
		&[
			("reader.txt", PLAIN),
			(
				"source-sentry.lock",
				&lockfile(&[("reader.txt", &declaration)], &[]),
			),
		],
	);
	let before = tree.claim(&local("reader.txt")).declaration.clone();
	let planned = plan(&tree, "docs/guide.md", "archive/deep/guide.md").unwrap();
	assert!(
		planned
			.summary
			.sources
			.iter()
			.any(|source| source.kind == RenameSourceKind::UnmatchedPattern)
	);
	let after = tree.apply(&planned.plan);
	let sources = &after.claim(&local("reader.txt")).declaration.sources;
	let SourceDecl::Pattern(matching) = &sources[0] else {
		panic!("pattern");
	};
	assert_eq!(matching.members[0].path, "/archive/deep/guide.md");
	assert_eq!(matching.members[0].oid, oid(PLAIN));
	assert_eq!(sources[1], before.sources[1]);
}

/// Conflicting pattern baselines are never silently overwritten.
#[test]
fn a_reviewed_destination_member_refuses_the_rename() {
	let declaration = format!(
		r#"{{"sources": [{{"pattern": "/**/*.md", "members": [{}, {}]}}]}}"#,
		attested("/docs/guide.md", PLAIN),
		attested("/archive/deep/guide.md", "different\n")
	);
	let tree = moved(
		"{}",
		&[
			(
				"source-sentry.lock",
				&lockfile(&[("reader.txt", &declaration)], &[]),
			),
			("reader.txt", PLAIN),
		],
	);
	assert!(matches!(
		plan(&tree, "docs/guide.md", "archive/deep/guide.md"),
		Err(MutateError::RenameMemberConflict { .. })
	));
}

/// A sealed document can move only with its sealed content.
#[test]
fn a_changed_record_is_refused() {
	let tree = moved(&format!(r#"{{"seal": {}}}"#, seal_of("different\n")), &[]);
	let Err(MutateError::WouldIntroduce { diagnostics }) =
		plan(&tree, "docs/guide.md", "archive/deep/guide.md")
	else {
		panic!("broken seal must refuse");
	};
	assert!(
		diagnostics
			.iter()
			.any(|diagnostic| diagnostic.kind == DiagnosticKind::BrokenSeal)
	);
}

/// Joining, leaving and changing a version are previewed from parsed series
/// membership, including after the old file has disappeared.
#[test]
fn series_changes_are_described() {
	let config = series_config("records-only");
	for (old, new, before, after) in [
		("docs/guide.md", "proposals/v1.md", None, Some("1")),
		("proposals/v1.md", "docs/guide.md", Some("1"), None),
		("proposals/v1.md", "proposals/v2.md", Some("1"), Some("2")),
	] {
		let tree = Tree::new(&[
			(".source-sentry/source-sentry.yaml", &config),
			("source-sentry.lock", &lockfile(&[(old, "{}")], &[])),
			(new, PLAIN),
		]);
		let planned = plan(&tree, old, new).unwrap();
		assert_eq!(
			planned
				.summary
				.series_before
				.map(|series| series.version.to_string()),
			before.map(str::to_owned)
		);
		assert_eq!(
			planned
				.summary
				.series_after
				.map(|series| series.version.to_string()),
			after.map(str::to_owned)
		);
	}
}

/// A rename cannot promote a sealed predecessor to the living head.
#[test]
fn a_rename_that_exposes_a_sealed_head_is_refused() {
	let declaration = format!(r#"{{"seal": {}}}"#, seal_of(PLAIN));
	let tree = Tree::new(&[
		(
			".source-sentry/source-sentry.yaml",
			&series_config("living"),
		),
		(
			"source-sentry.lock",
			&lockfile(
				&[("proposals/v1.md", &declaration), ("proposals/v2.md", "{}")],
				&[],
			),
		),
		("proposals/v1.md", PLAIN),
		("archive/guide.md", PLAIN),
	]);
	let Err(MutateError::WouldIntroduce { diagnostics }) =
		plan(&tree, "proposals/v2.md", "archive/guide.md")
	else {
		panic!("sealed head must refuse");
	};
	assert!(
		diagnostics
			.iter()
			.any(|diagnostic| diagnostic.kind == DiagnosticKind::SealedHead)
	);
}

/// A destination already invalid in discovery cannot be adopted by rename.
#[test]
fn duplicate_versions_refuse_even_if_the_destination_was_already_unclaimed() {
	let tree = Tree::new(&[
		(
			".source-sentry/source-sentry.yaml",
			&series_config("records-only"),
		),
		(
			"source-sentry.lock",
			&lockfile(&[("docs/guide.md", "{}")], &[]),
		),
		("proposals/v1.md", PLAIN),
		("proposals/v01.md", PLAIN),
	]);
	assert!(matches!(
		plan(&tree, "docs/guide.md", "proposals/v01.md"),
		Err(MutateError::WouldIntroduce { .. })
	));
}

/// The old file must be absent and declared; the new one must be present
/// and unclaimed. Remote identities are refused by the core too.
#[test]
fn invalid_rename_preconditions_are_refused() {
	let tree = moved("{}", &[("docs/existing.md", PLAIN)]);
	for (old, new) in [
		(local("docs/existing.md"), local("archive/deep/guide.md")),
		(local("docs/guide.md"), local("missing.md")),
		(local("unknown.md"), local("archive/deep/guide.md")),
		(remote("abstract:video"), local("archive/deep/guide.md")),
		(local("docs/guide.md"), remote("abstract:video")),
	] {
		assert!(
			rename(
				&tree.snapshot,
				&FakeHistory::new(),
				&tree.configs,
				&tree.inventory,
				&RenameRequest { old, new }
			)
			.is_err()
		);
	}
	let claimed = moved(
		"{}",
		&[(
			"archive/deep/source-sentry.lock",
			&lockfile(&[("guide.md", "{}")], &[]),
		)],
	);
	assert!(matches!(
		plan(&claimed, "docs/guide.md", "archive/deep/guide.md"),
		Err(MutateError::RenameRefused { .. })
	));
}

/// Relative patterns that cannot be safely re-expressed refuse the plan.
#[test]
fn a_destination_directory_with_glob_metacharacters_refuses_patterns() {
	let tree = moved(
		r#"{"sources": [{"pattern": "/src/*.rs"}]}"#,
		&[("archive/[draft]/guide.md", PLAIN)],
	);
	// Rooted patterns do not need a new literal prefix.
	plan(&tree, "docs/guide.md", "archive/[draft]/guide.md").unwrap();
	let tree = moved(
		r#"{"sources": [{"pattern": "../src/*.rs"}]}"#,
		&[("archive/[draft]/guide.md", PLAIN)],
	);
	assert!(matches!(
		plan(&tree, "docs/guide.md", "archive/[draft]/guide.md"),
		Err(MutateError::CannotRenameSource { .. })
	));
}
