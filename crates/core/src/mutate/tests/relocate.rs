//! `track --from`.

use super::{
	BASE, REPOSITORY, REVIEW, SOURCE, Tree, attested, local, lockfile, markdown,
	path, remote, seal_of,
};
use crate::adapters::lockfile::LocalLockfile;
use crate::claims::Mode;
use crate::evaluate::settings::SettingOrigin;
use crate::model::DocumentId;
use crate::mutate::{
	CarriedTombstone, MutateError, Placement, Setting, Target, TrackFromRequest,
	track_from,
};
use crate::ports::{PlannedDelete, Snapshot};
use crate::testing::fake_oid;

/// A move of `documents` from `from` to `to`.
fn request(
	documents: &[DocumentId],
	from: Placement,
	to: Target,
) -> TrackFromRequest {
	TrackFromRequest {
		documents: documents.to_vec(),
		from,
		to,
	}
}

/// A local lockfile placement.
fn lockfile_in(directory: &str) -> Placement {
	Placement::Lockfile {
		directory: path(directory),
	}
}

/// The lockfile target, by default.
fn nearest() -> Target {
	Target::Lockfile { directory: None }
}

/// The declaration lines of an attested, reviewed on-file guide. A string
/// continuation would drop the first line's indentation, so each line is
/// written out.
const DECLARATION: &str = concat!(
	"  reviewed:\n",
	"    at: 2026-11-01T00:00:00Z\n",
	"    by: 'Bruno Costa'\n",
	"  classes: ['privacy']\n",
	"  sources:\n",
	"    - path: '/src/a.rs'\n",
	"      oid: 'OID'\n",
	"      reviewed:\n",
	"        at: 2026-11-01T00:00:00Z\n",
	"        by: 'Bruno Costa'\n",
);

/// A tree whose `docs/guide.md` is tracked on-file, attested.
fn on_file() -> Tree {
	let declaration = DECLARATION.replace("OID", &super::oid(SOURCE).to_string());
	Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("docs/guide.md", &markdown(&declaration)),
	])
}

/// A move between modes keeps every attestation and
/// review.
#[test]
fn on_file_to_a_lockfile_and_back() {
	let tree = on_file();
	let document = local("docs/guide.md");
	let original = tree.claim(&document).declaration.clone();
	let planned = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			std::slice::from_ref(&document),
			Placement::OnFile,
			nearest(),
		),
	)
	.unwrap();
	assert_eq!(planned.plan.writes.len(), 2);
	assert_eq!(planned.plan.deletes, [] as [PlannedDelete; 0]);
	let moved = &planned.summary.moved[0];
	assert!(moved.created);
	assert_eq!(moved.to.file, path("docs/source-sentry.lock"));
	// The directory's defaults are the same, so only origins change.
	assert_eq!(moved.settings.changed, [] as [Setting; 0]);
	assert_eq!(
		moved.settings.after.classes.origin,
		SettingOrigin::Declaration {
			file: path("docs/source-sentry.lock"),
			field: "documents[\"guide.md\"].classes".to_owned(),
		}
	);

	let tree = tree.apply(&planned.plan);
	tree.assert_clean();
	let claim = tree.claim(&document);
	assert_eq!(claim.mode, Mode::LocalLockfile);
	assert_eq!(claim.declaration, original);
	assert_eq!(
		tree.text("docs/guide.md"),
		"---\ntitle: 'Guide'\n---\n# Guide\n\nBody.\n"
	);

	let planned = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			std::slice::from_ref(&document),
			lockfile_in("docs"),
			Target::OnFile,
		),
	)
	.unwrap();
	// The lockfile is left empty, so it is deleted.
	let lockfile = path("docs/source-sentry.lock");
	let current = tree.snapshot.read(&lockfile).unwrap();
	assert_eq!(
		planned.plan.deletes,
		[PlannedDelete {
			path: lockfile.clone(),
			expected: fake_oid(&current),
		}]
	);
	let tree = tree.apply(&planned.plan);
	tree.assert_clean();
	assert!(!tree.snapshot.paths().unwrap().contains(&lockfile));
	let claim = tree.claim(&document);
	assert_eq!(claim.mode, Mode::OnFile);
	assert_eq!(claim.declaration, original);
}

/// Moving every entry of a nested lockfile to its parent
/// merges it, with its tombstones, and deletes it.
#[test]
fn merging_a_nested_lockfile_into_its_parent() {
	let first =
		format!(r#"{{"retired": {REVIEW}, "seal": {}}}"#, seal_of("old\n"));
	let second =
		format!(r#"{{"retired": {REVIEW}, "seal": {}}}"#, seal_of("later\n"));
	let tombstone = format!("{first}, {second}");
	let a = format!(r#"{{"sources": [{}]}}"#, attested("/src/a.rs", SOURCE));
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		("docs/x.md", "# X\n"),
		("other/c.md", "# C\n"),
		(
			"source-sentry.lock",
			&lockfile(&[("other/c.md", "{}")], &[]),
		),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", &a), ("b.md", "{}")], &[("old.md", &tombstone)]),
		),
	]);
	let parent = Target::Lockfile {
		directory: Some(path("")),
	};
	let attempt = |documents: &[DocumentId], from: Placement| {
		track_from(
			&tree.snapshot,
			&tree.configs,
			&tree.inventory,
			&request(documents, from, parent.clone()),
		)
	};

	// Moving one entry would leave the other behind the nested lockfile.
	let error = attempt(&[local("docs/a.md")], lockfile_in("docs")).unwrap_err();
	let MutateError::StrandedEntries {
		lockfile,
		directory,
		documents,
	} = &error
	else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(lockfile, &path("docs/source-sentry.lock"));
	assert_eq!(directory, &path("docs"));
	assert_eq!(documents, &[local("docs/b.md")]);

	let both = [local("docs/a.md"), local("docs/b.md")];
	let planned = attempt(&both, lockfile_in("docs")).unwrap();
	let nested = path("docs/source-sentry.lock");
	let current = tree.snapshot.read(&nested).unwrap();
	assert_eq!(
		planned.plan.deletes,
		[PlannedDelete {
			path: nested.clone(),
			expected: fake_oid(&current),
		}]
	);
	let [write] = planned.plan.writes.as_slice() else {
		panic!("expected one write");
	};
	assert_eq!(write.path, path("source-sentry.lock"));
	assert_eq!(
		planned.summary.carried,
		[CarriedTombstone {
			document: local("docs/old.md"),
			from: nested.clone(),
			to: path("source-sentry.lock"),
		}]
	);

	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert!(!after.snapshot.paths().unwrap().contains(&nested));
	for document in &both {
		let claim = after.claim(document);
		assert_eq!(claim.declared_in, path("source-sentry.lock"));
		assert_eq!(claim.declaration, tree.claim(document).declaration);
	}
	let bytes = after.snapshot.read(&path("source-sentry.lock")).unwrap();
	let merged = LocalLockfile::parse(&path(""), &bytes).unwrap();
	let tombstones = &merged.retired[&path("docs/old.md")];
	assert_eq!(tombstones.len(), 2);
	assert_eq!(
		serde_json::to_value(&tombstones[0]).unwrap(),
		serde_json::from_str::<serde_json::Value>(&first).unwrap()
	);
	assert_eq!(
		serde_json::to_value(&tombstones[1]).unwrap(),
		serde_json::from_str::<serde_json::Value>(&second).unwrap()
	);
	assert!(merged.documents.contains_key(&path("other/c.md")));
}

/// A lockfile in between that the change does not empty keeps its
/// authority.
#[test]
fn a_nearer_lockfile_that_stays_is_refused() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/a.md", "# A\n"),
		("docs/guide.md", &markdown("  classes: ['privacy']\n")),
		("source-sentry.lock", &lockfile(&[], &[])),
		("docs/source-sentry.lock", &lockfile(&[("a.md", "{}")], &[])),
	]);
	let error = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			&[local("docs/guide.md")],
			Placement::OnFile,
			Target::Lockfile {
				directory: Some(path("")),
			},
		),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::NearerLockfile { .. }),
		"{error:?}"
	);
}

/// Splitting a lockfile moves every entry the new one
/// takes over, and carries the tombstones below it.
#[test]
fn splitting_a_lockfile_moves_entries_and_tombstones() {
	let first =
		format!(r#"{{"retired": {REVIEW}, "seal": {}}}"#, seal_of("old\n"));
	let second =
		format!(r#"{{"retired": {REVIEW}, "seal": {}}}"#, seal_of("later\n"));
	let tombstone = format!("{first}, {second}");
	let a = format!(r#"{{"sources": [{}]}}"#, attested("/src/a.rs", SOURCE));
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("docs/a.md", "# A\n"),
		("docs/b.md", "# B\n"),
		("other/c.md", "# C\n"),
		(
			"source-sentry.lock",
			&lockfile(
				&[("docs/a.md", &a), ("docs/b.md", "{}"), ("other/c.md", "{}")],
				&[("docs/old.md", &tombstone)],
			),
		),
	]);
	let to = Target::Lockfile {
		directory: Some(path("docs")),
	};

	let error = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(&[local("docs/a.md")], lockfile_in(""), to.clone()),
	)
	.unwrap_err();
	let MutateError::StrandedEntries { documents, .. } = &error else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(documents, &[local("docs/b.md")]);

	let both = [local("docs/a.md"), local("docs/b.md")];
	let planned = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(&both, lockfile_in(""), to),
	)
	.unwrap();
	assert_eq!(planned.summary.carried.len(), 1);
	assert_eq!(planned.summary.carried[0].document, local("docs/old.md"));
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(
		after.claim(&local("docs/a.md")).declaration,
		tree.claim(&local("docs/a.md")).declaration
	);
	assert_eq!(
		after.claim(&local("other/c.md")).declared_in,
		path("source-sentry.lock")
	);
	let bytes = after
		.snapshot
		.read(&path("docs/source-sentry.lock"))
		.unwrap();
	let nested = LocalLockfile::parse(&path("docs"), &bytes).unwrap();
	let tombstones = &nested.retired[&path("docs/old.md")];
	assert_eq!(tombstones.len(), 2);
	assert_eq!(
		serde_json::to_value(&tombstones[0]).unwrap(),
		serde_json::from_str::<serde_json::Value>(&first).unwrap()
	);
	assert_eq!(
		serde_json::to_value(&tombstones[1]).unwrap(),
		serde_json::from_str::<serde_json::Value>(&second).unwrap()
	);
}

/// Moving an entry to the nearest lockfile fixes its authority.
#[test]
fn an_entry_outside_its_authority_moves_to_the_nearest_lockfile() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/a.md", "# A\n"),
		("source-sentry.lock", &lockfile(&[("docs/a.md", "{}")], &[])),
		("docs/source-sentry.lock", &lockfile(&[], &[])),
	]);
	assert!(tree.inventory.has_errors());
	let planned = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(&[local("docs/a.md")], lockfile_in(""), nearest()),
	)
	.unwrap();
	assert!(!planned.summary.moved[0].created);
	// The root's lockfile is left empty, so it is deleted.
	assert_eq!(planned.plan.deletes.len(), 1);
	assert_eq!(planned.plan.deletes[0].path, path("source-sentry.lock"));
	tree.apply(&planned.plan).assert_clean();
}

/// A transfer between owners previews the defaults that change,
/// and keeps the entry's tombstone with it.
#[test]
fn between_owners_with_a_preview() {
	let tombstone = format!(r#"{{"retired": {REVIEW}}}"#);
	let entry = r#"{"sources": [{"path": "/src/a.rs"}]}"#;
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		(
			".source-sentry/remotes/legal.yaml",
			"schedule:\n  grace: 14\n",
		),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(
				&[("abstract:video", entry)],
				&[("abstract:video", &tombstone)],
			),
		),
	]);
	let document = remote("abstract:video");
	let planned = track_from(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			std::slice::from_ref(&document),
			Placement::Remote {
				project: crate::model::RepoPath::ROOT,
				owner: "legal".to_owned(),
			},
			Target::Remote {
				project: crate::model::RepoPath::ROOT,
				owner: "marketing".to_owned(),
			},
		),
	)
	.unwrap();
	let moved = &planned.summary.moved[0];
	assert_eq!(moved.settings.changed, [Setting::Grace]);
	assert_eq!(moved.settings.before.grace.value, 14);
	assert_eq!(moved.settings.after.grace.value, 0);
	assert_eq!(moved.settings.after.grace.origin, SettingOrigin::BuiltIn);
	assert_eq!(planned.summary.carried.len(), 1);
	// The entry and its tombstone both leave, so legal's lockfile is deleted
	// and its paired config stays.
	assert_eq!(planned.plan.deletes.len(), 1);
	assert_eq!(
		planned.plan.deletes[0].path,
		path(".source-sentry/remotes/legal.lock")
	);

	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let claim = after.claim(&document);
	assert_eq!(
		claim.mode,
		Mode::RemoteLockfile {
			owner: "marketing".to_owned(),
		}
	);
	assert_eq!(after.inventory.tombstones.len(), 1);
	assert_eq!(
		after.inventory.tombstones[0].lockfile,
		path(".source-sentry/remotes/marketing.lock")
	);
}

#[test]
fn refusals() {
	let entry = format!(r#"{{"seal": {}}}"#, seal_of("# A\n"));
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/a.md", "# A\n"),
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", &entry)], &[]),
		),
	]);
	let attempt = |request: &TrackFromRequest| {
		track_from(&tree.snapshot, &tree.configs, &tree.inventory, request)
			.unwrap_err()
	};
	let document = local("docs/a.md");

	// Records are lockfile-only.
	let error = attempt(&request(
		std::slice::from_ref(&document),
		lockfile_in("docs"),
		Target::OnFile,
	));
	assert!(
		matches!(error, MutateError::RecordToOnFile { .. }),
		"{error:?}"
	);

	let error = attempt(&request(
		std::slice::from_ref(&document),
		Placement::OnFile,
		nearest(),
	));
	assert!(
		matches!(error, MutateError::WrongOrigin { .. }),
		"{error:?}"
	);

	let error = attempt(&request(
		std::slice::from_ref(&document),
		lockfile_in("docs"),
		nearest(),
	));
	assert!(
		matches!(error, MutateError::SamePlacement { .. }),
		"{error:?}"
	);

	let error = attempt(&request(&[], lockfile_in("docs"), nearest()));
	assert!(matches!(error, MutateError::NothingToMove), "{error:?}");

	let error = attempt(&request(
		&[document.clone(), document],
		lockfile_in("docs"),
		nearest(),
	));
	assert!(
		matches!(error, MutateError::RepeatedDocument { .. }),
		"{error:?}"
	);
}
