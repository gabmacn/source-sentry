//! `attest`.

use super::{
	BASE, REPOSITORY, REVIEWER, SOURCE, Tree, at, attested, local, lockfile,
	markdown, oid, path, present, review, seal_of, series_config,
};
use crate::model::{DocumentId, Member, PathSource, PatternSource, SourceDecl};
use crate::mutate::{
	AttestRequest, AttestedEntry, Expectation, Mismatch, MutateError, Selection,
	attest,
};
use crate::ports::Canonical;

/// An attestation of every source of `document`, without expectations.
fn request(document: DocumentId) -> AttestRequest {
	AttestRequest {
		document,
		selection: Selection::All,
		expectations: None,
		reviewer: Some(REVIEWER.to_owned()),
		at: at(),
		reread: false,
	}
}

/// A tree whose `docs/guide.md` declares `/src/a.rs` on-file.
fn on_file() -> Tree {
	Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		(
			"docs/guide.md",
			&markdown("  sources:\n    - path: '/src/a.rs'\n"),
		),
	])
}

/// A tree whose `docs/source-sentry.lock` lists `guide.md` with `/src/a.rs`
/// and the pattern `/db/*.sql`, which matches two files.
fn in_lockfile(members: &str) -> Tree {
	let entry = format!(
		r#"{{"sources": [{{"path": "/src/a.rs"}}, {{"members": [{members}], "pattern": "/db/*.sql"}}]}}"#
	);
	Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("db/1.sql", "one\n"),
		("db/2.sql", "two\n"),
		("docs/guide.md", "# Guide\n"),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("guide.md", &entry)], &[]),
		),
	])
}

/// The declared sources of `document` in `tree`.
fn sources(tree: &Tree, document: &str) -> Vec<SourceDecl> {
	tree.claim(&local(document)).declaration.sources.clone()
}

/// The first pattern source of `document` in `tree`. Lockfiles sort sources
/// by their declared path or pattern, so position says little.
fn pattern_of(tree: &Tree, document: &str) -> PatternSource {
	sources(tree, document)
		.into_iter()
		.find_map(|source| match source {
			SourceDecl::Pattern(pattern) => Some(pattern),
			SourceDecl::Path(_) => None,
		})
		.expect("a pattern source")
}

/// The first path source of `document` in `tree`.
fn path_of(tree: &Tree, document: &str) -> PathSource {
	sources(tree, document)
		.into_iter()
		.find_map(|source| match source {
			SourceDecl::Path(path) => Some(path),
			SourceDecl::Pattern(_) => None,
		})
		.expect("a path source")
}

#[test]
fn on_file_records_current_oids_and_keeps_other_bytes() {
	let tree = on_file();
	let planned = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap();
	assert_eq!(planned.plan.writes.len(), 1);
	assert_eq!(planned.plan.writes[0].path, path("docs/guide.md"));
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let text = after.text("docs/guide.md");
	assert!(text.starts_with("---\ntitle: 'Guide'\n"), "{text}");
	assert!(text.ends_with("---\n# Guide\n\nBody.\n"), "{text}");
	let declared = sources(&after, "docs/guide.md");
	let [SourceDecl::Path(source)] = declared.as_slice() else {
		panic!("expected one path source");
	};
	assert_eq!(source.oid, Some(oid(SOURCE)));
	assert_eq!(source.reviewed, Some(review()));
	// Without the reread option, the document's own date is untouched.
	assert_eq!(
		after.claim(&local("docs/guide.md")).declaration.reviewed,
		None
	);
	assert_eq!(planned.summary.expectations, [present("src/a.rs", SOURCE)]);
	assert_eq!(planned.summary.review, review());
	assert_eq!(
		planned.summary.sources[0].entries,
		[AttestedEntry {
			path: path("src/a.rs"),
			previous: None,
			recorded: Some(oid(SOURCE)),
		}]
	);
}

#[test]
fn in_a_lockfile_with_matching_expectations() {
	let tree = in_lockfile("");
	let mut request = request(local("docs/guide.md"));
	request.expectations = Some(vec![
		present("db/2.sql", "two\n"),
		present("src/a.rs", SOURCE),
		present("db/1.sql", "one\n"),
	]);
	let planned = attest(&tree.snapshot, &tree.inventory, &request).unwrap();
	assert_eq!(planned.plan.writes[0].path, path("docs/source-sentry.lock"));
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(
		pattern_of(&after, "docs/guide.md").members,
		[
			Member {
				oid: oid("one\n"),
				path: "/db/1.sql".to_owned(),
				reviewed: review(),
			},
			Member {
				oid: oid("two\n"),
				path: "/db/2.sql".to_owned(),
				reviewed: review(),
			},
		]
	);
}

/// A match the reviewer did not see refuses the attestation.
#[test]
fn an_unseen_pattern_addition_is_a_mismatch() {
	let tree = in_lockfile("");
	let tree = Tree::of(tree.snapshot.with_file("db/3.sql", b"three\n"));
	let mut request = request(local("docs/guide.md"));
	request.expectations = Some(vec![
		present("src/a.rs", SOURCE),
		present("db/1.sql", "one\n"),
		present("db/2.sql", "two\n"),
	]);
	let error = attest(&tree.snapshot, &tree.inventory, &request).unwrap_err();
	let MutateError::ExpectationMismatch { mismatches, .. } = error else {
		panic!("expected a mismatch, got {error:?}");
	};
	assert_eq!(
		mismatches,
		[Mismatch::Unexpected(present("db/3.sql", "three\n"))]
	);
}

#[test]
fn a_changed_or_unselected_expectation_is_a_mismatch() {
	let tree = in_lockfile("");
	let mut request = request(local("docs/guide.md"));
	request.selection = Selection::Sources(vec!["/src/a.rs".to_owned()]);
	request.expectations = Some(vec![
		present("src/a.rs", "an older version\n"),
		present("db/1.sql", "one\n"),
	]);
	let error = attest(&tree.snapshot, &tree.inventory, &request).unwrap_err();
	let MutateError::ExpectationMismatch { mismatches, .. } = error else {
		panic!("expected a mismatch, got {error:?}");
	};
	assert_eq!(
		mismatches,
		[
			Mismatch::Differs {
				expected: present("db/1.sql", "one\n"),
				actual: None,
			},
			Mismatch::Differs {
				expected: present("src/a.rs", "an older version\n"),
				actual: Some(present("src/a.rs", SOURCE)),
			},
		]
	);
}

/// Only a pattern's membership acknowledges a removal, and the
/// expectations must name it.
#[test]
fn a_removal_is_acknowledged_through_the_pattern() {
	let gone = attested("/db/0.sql", "zero\n");
	let tree = in_lockfile(&gone);
	let mut request = request(local("docs/guide.md"));
	request.selection = Selection::Sources(vec!["/db/*.sql".to_owned()]);
	request.expectations = Some(vec![
		present("db/1.sql", "one\n"),
		present("db/2.sql", "two\n"),
	]);
	let error = attest(&tree.snapshot, &tree.inventory, &request).unwrap_err();
	let MutateError::ExpectationMismatch { mismatches, .. } = error else {
		panic!("expected a mismatch, got {error:?}");
	};
	let removal = Expectation::Absent {
		path: path("db/0.sql"),
	};
	assert_eq!(mismatches, [Mismatch::Unexpected(removal.clone())]);

	request.expectations.as_mut().unwrap().push(removal);
	let planned = attest(&tree.snapshot, &tree.inventory, &request).unwrap();
	let entries = &planned.summary.sources[0].entries;
	assert_eq!(entries[0].path, path("db/0.sql"));
	assert_eq!(entries[0].recorded, None);
	let after = tree.apply(&planned.plan);
	assert_eq!(pattern_of(&after, "docs/guide.md").members.len(), 2);
	// The path source was not selected, so it stays unattested.
	assert_eq!(path_of(&after, "docs/guide.md").oid, None);
}

#[test]
fn the_reread_option_alone_updates_the_document_review() {
	let tree = on_file();
	let mut request = request(local("docs/guide.md"));
	request.reread = true;
	let planned = attest(&tree.snapshot, &tree.inventory, &request).unwrap();
	assert!(planned.summary.reread);
	let after = tree.apply(&planned.plan);
	assert_eq!(
		after.claim(&local("docs/guide.md")).declaration.reviewed,
		Some(review())
	);
}

/// A `living` series entry completes when every source
/// is attested.
#[test]
fn a_living_series_entry_completes_once_fully_attested() {
	let entry = r#"{"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]}"#;
	let tree = Tree::new(&[
		(REPOSITORY, &series_config("living")),
		("src/a.rs", SOURCE),
		("src/b.rs", "fn b() {}\n"),
		("proposals/v1.md", "# v1\n"),
		(
			"proposals/source-sentry.lock",
			&lockfile(&[("v1.md", entry)], &[]),
		),
	]);
	let document = local("proposals/v1.md");

	// One source of two leaves the entry incomplete.
	let mut partial = request(document.clone());
	partial.selection = Selection::Sources(vec!["/src/a.rs".to_owned()]);
	let planned = attest(&tree.snapshot, &tree.inventory, &partial).unwrap();
	assert_eq!(planned.summary.completed, None);
	let tree = tree.apply(&planned.plan);
	assert_eq!(tree.claim(&document).declaration.completed, None);

	// The second completes it, since the first is still fresh.
	let mut rest = request(document.clone());
	rest.selection = Selection::Sources(vec!["/src/b.rs".to_owned()]);
	let planned = attest(&tree.snapshot, &tree.inventory, &rest).unwrap();
	assert_eq!(planned.summary.completed, Some(review()));
	let tree = tree.apply(&planned.plan);
	assert_eq!(tree.claim(&document).declaration.completed, Some(review()));
	tree.assert_clean();

	// Completion is recorded once and never recomputed.
	let mut again = request(document.clone());
	again.at = "2026-12-01T00:00:00Z".parse().unwrap();
	let planned = attest(&tree.snapshot, &tree.inventory, &again).unwrap();
	assert_eq!(planned.summary.completed, None);
	let tree = tree.apply(&planned.plan);
	assert_eq!(tree.claim(&document).declaration.completed, Some(review()));
}

#[test]
fn a_records_only_entry_never_gets_completed() {
	let entry = r#"{"sources": [{"path": "/src/a.rs"}]}"#;
	let tree = Tree::new(&[
		(REPOSITORY, &series_config("records-only")),
		("src/a.rs", SOURCE),
		("proposals/v1.md", "# v1\n"),
		(
			"proposals/source-sentry.lock",
			&lockfile(&[("v1.md", entry)], &[]),
		),
	]);
	let planned = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("proposals/v1.md")),
	)
	.unwrap();
	assert_eq!(planned.summary.completed, None);
}

/// A relative pattern's members are written relative to the
/// document.
#[test]
fn a_relative_pattern_keeps_relative_members() {
	let entry = r#"{"sources": [{"pattern": "../src/*.rs"}]}"#;
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("docs/guide.md", "# Guide\n"),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("guide.md", entry)], &[]),
		),
	]);
	let planned = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap();
	let after = tree.apply(&planned.plan);
	assert_eq!(
		pattern_of(&after, "docs/guide.md").members[0].path,
		"../src/a.rs"
	);
}

/// `attest` never touches a record.
#[test]
fn a_sealed_record_is_refused() {
	let entry = format!(
		r#"{{"seal": {}, "sources": [{}]}}"#,
		seal_of("# Audit\n"),
		attested("/src/a.rs", SOURCE)
	);
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", "changed\n"),
		("audits/a.md", "# Audit\n"),
		(
			"audits/source-sentry.lock",
			&lockfile(&[("a.md", &entry)], &[]),
		),
	]);
	let error = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("audits/a.md")),
	)
	.unwrap_err();
	assert!(matches!(error, MutateError::Sealed { .. }), "{error:?}");
}

/// No oid is guessed for a filtered source.
#[test]
fn a_source_without_a_canonical_oid_is_refused() {
	let tree = on_file();
	let canonical = Canonical::UnsupportedFilter {
		filter: "crypt".to_owned(),
		attribute_origin: ".gitattributes:1".to_owned(),
	};
	let tree =
		Tree::of(tree.snapshot.with_canonical("src/a.rs", canonical.clone()));
	let error = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap_err();
	let MutateError::NotCanonical {
		path: file,
		canonical: found,
	} = &error
	else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(file, &path("src/a.rs"));
	assert_eq!(found, &canonical);
	assert!(error.to_string().contains("--rev"), "{error}");
}

/// A new pattern that matches nothing is never recorded.
#[test]
fn a_new_empty_pattern_is_refused() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		(
			"docs/guide.md",
			&markdown("  sources:\n    - pattern: '/db/*.sql'\n"),
		),
	]);
	let error = attest(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::EmptyPattern { .. }),
		"{error:?}"
	);
}

/// An explicitly named source that is missing is an error, but
/// only when it is selected.
#[test]
fn a_missing_source_refuses_only_when_selected() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		(
			"docs/guide.md",
			&markdown(
				"  sources:\n    - path: '/src/a.rs'\n    - path: '/src/gone.rs'\n",
			),
		),
	]);
	let document = local("docs/guide.md");
	let error =
		attest(&tree.snapshot, &tree.inventory, &request(document.clone()))
			.unwrap_err();
	assert!(
		matches!(error, MutateError::MissingSource { .. }),
		"{error:?}"
	);

	let mut selected = request(document);
	selected.selection = Selection::Sources(vec!["/src/a.rs".to_owned()]);
	assert!(attest(&tree.snapshot, &tree.inventory, &selected).is_ok());
}

#[test]
fn other_refusals() {
	let tree = on_file();
	let document = local("docs/guide.md");

	let mut unknown = request(document.clone());
	unknown.selection = Selection::Sources(vec!["/src/b.rs".to_owned()]);
	let error = attest(&tree.snapshot, &tree.inventory, &unknown).unwrap_err();
	assert!(
		matches!(error, MutateError::UnknownSource { .. }),
		"{error:?}"
	);

	let mut email = request(document.clone());
	email.reviewer = Some("ana@example.com".to_owned());
	let error = attest(&tree.snapshot, &tree.inventory, &email).unwrap_err();
	assert!(
		matches!(error, MutateError::InvalidReviewer { .. }),
		"{error:?}"
	);

	let error =
		attest(&tree.snapshot, &tree.inventory, &request(local("src/a.rs")))
			.unwrap_err();
	assert!(matches!(error, MutateError::NotTracked { .. }), "{error:?}");

	let empty = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/guide.md", &markdown("  classes: ['privacy']\n")),
	]);
	let error = attest(
		&empty.snapshot,
		&empty.inventory,
		&request(document.clone()),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::NothingToAttest { .. }),
		"{error:?}"
	);
	let mut reread = request(document);
	reread.reread = true;
	assert!(attest(&empty.snapshot, &empty.inventory, &reread).is_ok());
}
