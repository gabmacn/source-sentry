//! `retire`.

use super::{
	BASE, REPOSITORY, SOURCE, Tree, at, attested, local, lockfile, markdown,
	path, remote, review, seal_of, series_config,
};
use crate::adapters::lockfile::{LocalLockfile, RemoteLockfile};
use crate::model::{DocumentId, Retired};
use crate::mutate::{MutateError, RetireRequest, retire};
use crate::ports::Snapshot;

/// A retirement of `document` by the tests' reviewer.
fn request(document: DocumentId) -> RetireRequest {
	RetireRequest {
		document,
		by: Some(super::REVIEWER.to_owned()),
		at: at(),
	}
}

#[test]
fn a_sealed_local_record_keeps_its_seal() {
	let entry = format!(
		r#"{{"seal": {}, "sources": [{}]}}"#,
		seal_of("# Audit\n"),
		attested("/src/a.rs", SOURCE)
	);
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("audits/a.md", "# Audit\n"),
		("audits/b.md", "# Audit\n"),
		(
			"audits/source-sentry.lock",
			&lockfile(&[("a.md", &entry), ("b.md", "{}")], &[]),
		),
	]);
	let planned = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(local("audits/a.md")),
	)
	.unwrap();
	let seal = tree.claim(&local("audits/a.md")).declaration.seal.clone();
	assert!(seal.is_some());
	let tombstone = Retired {
		retired: review(),
		seal,
	};
	assert_eq!(planned.summary.tombstone, tombstone);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let bytes = after
		.snapshot
		.read(&path("audits/source-sentry.lock"))
		.unwrap();
	let written = LocalLockfile::parse(&path("audits"), &bytes).unwrap();
	assert!(!written.documents.contains_key(&path("audits/a.md")));
	assert!(written.documents.contains_key(&path("audits/b.md")));
	assert_eq!(written.retired.get(&path("audits/a.md")), Some(&tombstone));
}

/// Retiring is the remedy for a listed document that is gone.
#[test]
fn a_missing_document_can_be_retired() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/a.md", "# A\n"),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("gone.md", "{}")], &[]),
		),
	]);
	assert!(tree.inventory.has_errors());
	let planned = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/gone.md")),
	)
	.unwrap();
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(after.inventory.tombstones.len(), 1);
}

#[test]
fn a_remote_document_is_retired_without_a_seal() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(
				&[("abstract:video", r#"{"sources": [{"path": "/src/a.rs"}]}"#)],
				&[],
			),
		),
	]);
	let planned = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(remote("abstract:video")),
	)
	.unwrap();
	assert_eq!(
		planned.summary.lockfile,
		path(".source-sentry/remotes/legal.lock")
	);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let bytes = after
		.snapshot
		.read(&path(".source-sentry/remotes/legal.lock"))
		.unwrap();
	let written = RemoteLockfile::parse(&bytes).unwrap();
	assert!(written.documents.is_empty());
	let identity = "abstract:video".parse().unwrap();
	assert_eq!(
		written.retired.get(&identity),
		Some(&Retired {
			retired: review(),
			seal: None,
		})
	);
}

/// Deleting an on-file document removes its claim, so it needs no
/// tombstone.
#[test]
fn an_on_file_document_needs_no_tombstone() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/guide.md", &markdown("  classes: ['privacy']\n")),
	]);
	let error = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::OnFileRetire { .. }),
		"{error:?}"
	);
	assert!(error.to_string().contains("deleting the file"), "{error}");
}

/// Retiring the living head requires retiring the series, whose
/// config the refusal names.
#[test]
fn the_living_head_is_refused_naming_the_config() {
	let tree = Tree::new(&[
		(REPOSITORY, &series_config("living")),
		("proposals/v1.md", "# v1\n"),
		(
			"proposals/source-sentry.lock",
			&lockfile(&[("v1.md", "{}")], &[]),
		),
	]);
	let error = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(local("proposals/v1.md")),
	)
	.unwrap_err();
	let MutateError::RetireLivingHead { config, .. } = &error else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(config.as_ref(), Some(&path(REPOSITORY)));
	assert!(error.to_string().contains(REPOSITORY), "{error}");
}

/// A tombstone is never replaced, even for a reused path.
#[test]
fn an_existing_tombstone_is_never_replaced() {
	let tombstone = format!(r#"{{"retired": {}}}"#, super::REVIEW);
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/a.md", "# A\n"),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", "{}")], &[("a.md", &tombstone)]),
		),
	]);
	let error = retire(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/a.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::AlreadyRetired { .. }),
		"{error:?}"
	);
}
