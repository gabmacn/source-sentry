//! `seal`.

use super::{
	BASE, REPOSITORY, SOURCE, Tree, at, attested, local, lockfile, markdown, oid,
	path, remote, seal_of, second, series_config,
};
use crate::model::{DocumentId, Seal};
use crate::mutate::{MutateError, SealRequest, seal};

/// A seal of `document` by Carla.
fn request(document: DocumentId) -> SealRequest {
	SealRequest {
		document,
		by: Some("Carla Mendes".to_owned()),
		at: at(),
	}
}

/// The contents of the audit.
const AUDIT: &str = "# Audit\n";

/// A tree whose `audits/source-sentry.lock` lists `a.md`, attested.
fn audits(extra: &str) -> Tree {
	let entry = format!(
		r#"{{{extra}"sources": [{}]}}"#,
		attested("/src/a.rs", SOURCE)
	);
	Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("audits/a.md", AUDIT),
		(
			"audits/source-sentry.lock",
			&lockfile(&[("a.md", &entry)], &[]),
		),
	])
}

#[test]
fn sealing_records_the_record_s_canonical_oid() {
	let tree = audits("");
	let planned = seal(
		&tree.snapshot,
		&tree.inventory,
		&request(local("audits/a.md")),
	)
	.unwrap();
	let expected = Seal {
		at: second(),
		by: Some("Carla Mendes".to_owned()),
		oid: oid(AUDIT),
	};
	assert_eq!(planned.summary.seal, expected);
	assert_eq!(planned.summary.lockfile, path("audits/source-sentry.lock"));
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let claim = after.claim(&local("audits/a.md"));
	assert_eq!(claim.declaration.seal, Some(expected));
	// The rest of the entry is unchanged.
	assert_eq!(
		claim.declaration.sources,
		tree.claim(&local("audits/a.md")).declaration.sources
	);
}

#[test]
fn an_on_file_document_must_move_first() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		(
			"docs/guide.md",
			&markdown("  sources:\n    - path: '/src/a.rs'\n"),
		),
	]);
	let error = seal(
		&tree.snapshot,
		&tree.inventory,
		&request(local("docs/guide.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::OnFileRecord { .. }),
		"{error:?}"
	);
	assert!(error.to_string().contains("track --from"), "{error}");
}

#[test]
fn a_sealed_record_is_never_resealed() {
	let tree = audits(&format!(r#""seal": {}, "#, seal_of(AUDIT)));
	let error = seal(
		&tree.snapshot,
		&tree.inventory,
		&request(local("audits/a.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::AlreadySealed { .. }),
		"{error:?}"
	);
}

#[test]
fn a_remote_document_cannot_be_a_record() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", "{}")], &[]),
		),
	]);
	let error = seal(
		&tree.snapshot,
		&tree.inventory,
		&request(remote("abstract:video")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::RemoteRecord { .. }),
		"{error:?}"
	);
}

/// A `living` series whose newest entry is sealed is an error.
#[test]
fn the_living_head_is_refused() {
	let tree = Tree::new(&[
		(REPOSITORY, &series_config("living")),
		("proposals/v1.md", "# v1\n"),
		("proposals/v2.md", "# v2\n"),
		(
			"proposals/source-sentry.lock",
			&lockfile(&[("v1.md", "{}"), ("v2.md", "{}")], &[]),
		),
	]);
	let error = seal(
		&tree.snapshot,
		&tree.inventory,
		&request(local("proposals/v2.md")),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::SealLivingHead { .. }),
		"{error:?}"
	);
	// The predecessor can be sealed.
	assert!(
		seal(
			&tree.snapshot,
			&tree.inventory,
			&request(local("proposals/v1.md"))
		)
		.is_ok()
	);
}
