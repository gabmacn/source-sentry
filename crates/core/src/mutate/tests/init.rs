//! `init`.

use super::{BASE, REPOSITORY, Tree, local, markdown, path};
use crate::claims::Unclaimed;
use crate::model::{ClassName, Declaration};
use crate::mutate::Registered;
use crate::mutate::{
	Candidates, InitRequest, MutateError, Placement, init, init_preview,
};
use crate::ports::PlannedWrite;

/// A repository whose `docs` is a root by config, with two unclaimed
/// documents and one on-file document, and a file outside every root.
fn adoption() -> Tree {
	Tree::new(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.yaml", "root: true\n"),
		("docs/a.md", "# A\n"),
		("docs/guides/b.md", "# B\n"),
		("docs/c.md", &markdown("  classes: ['privacy']\n")),
		("notes/x.md", "# X\n"),
	])
}

#[test]
fn the_preview_lists_roots_claims_and_candidates() {
	let tree = adoption();
	let preview = init_preview(&tree.configs, &tree.inventory);
	assert_eq!(preview.roots, [path("docs")]);
	assert_eq!(preview.claims.len(), 1);
	assert_eq!(preview.claims[0].document, local("docs/c.md"));
	assert_eq!(preview.claims[0].placement, Placement::OnFile);
	assert_eq!(
		preview.candidates,
		[
			Unclaimed {
				path: path("docs/a.md"),
				root: path("docs"),
			},
			Unclaimed {
				path: path("docs/guides/b.md"),
				root: path("docs"),
			},
		]
	);
}

/// Candidates are registered unattested, in one lockfile at their
/// root, and never reviewed.
#[test]
fn every_candidate_is_registered_unattested() {
	let tree = adoption();
	let request = InitRequest {
		classes: vec![ClassName::new("internal-docs").unwrap()],
		..InitRequest::default()
	};
	let planned =
		init(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	let [write] = planned.plan.writes.as_slice() else {
		panic!("expected one lockfile");
	};
	assert_eq!(write.path, path("docs/source-sentry.lock"));
	assert_eq!(write.expected, None);
	assert_eq!(planned.summary.registered.len(), 2);
	assert!(
		planned
			.summary
			.registered
			.iter()
			.all(|registered| registered.created)
	);

	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(after.inventory.unclaimed, [] as [Unclaimed; 0]);
	let expected = Declaration {
		classes: vec![ClassName::new("internal-docs").unwrap()],
		..Declaration::default()
	};
	assert_eq!(after.claim(&local("docs/a.md")).declaration, expected);
	assert_eq!(
		after.claim(&local("docs/guides/b.md")).declaration,
		expected
	);
}

#[test]
fn chosen_candidates_only() {
	let tree = adoption();
	let request = InitRequest {
		candidates: Candidates::Only(vec![path("docs/a.md")]),
		..InitRequest::default()
	};
	let planned =
		init(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	let after = tree.apply(&planned.plan);
	assert_eq!(after.inventory.unclaimed.len(), 1);
	assert_eq!(after.inventory.unclaimed[0].path, path("docs/guides/b.md"));

	let request = InitRequest {
		candidates: Candidates::Only(vec![path("notes/x.md")]),
		..InitRequest::default()
	};
	let error =
		init(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap_err();
	assert!(
		matches!(error, MutateError::NotACandidate { .. }),
		"{error:?}"
	);
}

#[test]
fn nothing_to_adopt_plans_nothing() {
	let tree = Tree::new(&[(REPOSITORY, BASE), ("notes/x.md", "# X\n")]);
	let planned = init(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&InitRequest::default(),
	)
	.unwrap();
	assert_eq!(planned.plan.writes, [] as [PlannedWrite; 0]);
	assert_eq!(planned.summary.registered, [] as [Registered; 0]);
}
