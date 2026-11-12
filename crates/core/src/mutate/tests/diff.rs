//! `diff`.

use super::{
	BASE, REPOSITORY, REVIEWER, Tree, at, attested, local, lockfile, oid, path,
	present,
};
use crate::mutate::{
	AttestRequest, Baseline, Current, Expectation, MutateError, Selection,
	attest, diff,
};
use crate::testing::{FakeHistory, FakeSnapshot};

/// A history whose only commit holds `src/a.rs` as `v1` and `db/0.sql`, and
/// a working tree where `src/a.rs` is `v2`, `db/0.sql` is gone and
/// `db/1.sql` is new. `docs/guide.md` was attested against the commit.
fn setup() -> (FakeHistory, Tree) {
	let mut history = FakeHistory::new();
	history.commit(
		"2026-11-01T00:00:00Z".parse().unwrap(),
		&[
			("src/a.rs", Some(b"v1\n".as_slice())),
			("db/0.sql", Some(b"zero\n".as_slice())),
		],
	);
	let entry = format!(
		r#"{{"sources": [{}, {{"members": [{}], "pattern": "/db/*.sql"}}]}}"#,
		attested("/src/a.rs", "v1\n"),
		attested("/db/0.sql", "zero\n"),
	);
	let snapshot = FakeSnapshot::checkout(&history)
		.with_file(REPOSITORY, BASE.as_bytes())
		.with_file("src/a.rs", b"v2\n")
		.without_file("db/0.sql")
		.with_file("db/1.sql", b"one\n")
		.with_file("docs/guide.md", b"# Guide\n")
		.with_file(
			"docs/source-sentry.lock",
			lockfile(&[("guide.md", &entry)], &[]).as_bytes(),
		);
	(history, Tree::of(snapshot))
}

#[test]
fn baselines_come_from_history_and_expectations_feed_attest() {
	let (history, tree) = setup();
	let document = local("docs/guide.md");
	let result = diff(
		&tree.snapshot,
		&history,
		&tree.inventory,
		&document,
		&Selection::All,
	)
	.unwrap();

	let source = &result.sources[0];
	assert_eq!(source.declared, "/src/a.rs");
	let [entry] = source.entries.as_slice() else {
		panic!("expected one entry");
	};
	assert_eq!(
		entry.baseline,
		Baseline::Available {
			oid: oid("v1\n"),
			content: b"v1\n".to_vec(),
		}
	);
	assert_eq!(
		entry.current,
		Current::Present {
			oid: oid("v2\n"),
			content: Some(b"v2\n".to_vec()),
		}
	);
	assert!(entry.is_changed());

	let pattern = &result.sources[1];
	assert!(pattern.pattern);
	let [removed, added] = pattern.entries.as_slice() else {
		panic!("expected two entries");
	};
	assert_eq!(removed.path, path("db/0.sql"));
	assert_eq!(removed.current, Current::Missing);
	assert_eq!(added.path, path("db/1.sql"));
	assert_eq!(added.baseline, Baseline::Unattested);

	assert_eq!(
		result.expectations,
		[
			Expectation::Absent {
				path: path("db/0.sql"),
			},
			present("db/1.sql", "one\n"),
			present("src/a.rs", "v2\n"),
		]
	);

	// The developer passes what `diff` emitted to `attest`.
	let request = AttestRequest {
		document,
		selection: Selection::All,
		expectations: Some(result.expectations),
		reviewer: Some(REVIEWER.to_owned()),
		at: at(),
		reread: false,
	};
	let planned = attest(&tree.snapshot, &tree.inventory, &request).unwrap();
	tree.apply(&planned.plan).assert_clean();
}

/// A blob missing from the object database is reported, not
/// guessed.
#[test]
fn a_missing_blob_is_baseline_unavailable() {
	let (mut history, tree) = setup();
	history.forget_blob(&oid("v1\n"));
	let result = diff(
		&tree.snapshot,
		&history,
		&tree.inventory,
		&local("docs/guide.md"),
		&Selection::Sources(vec!["/src/a.rs".to_owned()]),
	)
	.unwrap();
	assert_eq!(result.sources.len(), 1);
	assert_eq!(
		result.sources[0].entries[0].baseline,
		Baseline::Unavailable { oid: oid("v1\n") }
	);
	assert_eq!(result.expectations, [present("src/a.rs", "v2\n")]);
}

#[test]
fn an_untracked_document_is_refused() {
	let (history, tree) = setup();
	let error = diff(
		&tree.snapshot,
		&history,
		&tree.inventory,
		&local("docs/other.md"),
		&Selection::All,
	)
	.unwrap_err();
	assert!(matches!(error, MutateError::NotTracked { .. }), "{error:?}");
}
