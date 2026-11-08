//! Schema migration.

use super::{BASE, REPOSITORY, Tree, lockfile, path};
use crate::mutate::{LockfileSchema, MutateError, migrate};
use crate::ports::PlannedWrite;

#[test]
fn a_current_tree_plans_nothing() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.lock", &lockfile(&[], &[])),
		(".source-sentry/remotes/legal.lock", &lockfile(&[], &[])),
	]);
	let planned = migrate(&tree.snapshot, &tree.inventory).unwrap();
	assert_eq!(planned.plan.writes, [] as [PlannedWrite; 0]);
	assert_eq!(planned.summary.current, 1);
	assert_eq!(
		planned.summary.lockfiles,
		[
			LockfileSchema {
				path: path(".source-sentry/remotes/legal.lock"),
				schema: 1,
			},
			LockfileSchema {
				path: path("docs/source-sentry.lock"),
				schema: 1,
			},
		]
	);
}

#[test]
fn an_unknown_schema_is_refused() {
	let tree = Tree::new(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.lock",
			r#"{"documents": {}, "retired": {}, "schema": 2}"#,
		),
	]);
	let error = migrate(&tree.snapshot, &tree.inventory).unwrap_err();
	assert!(
		matches!(error, MutateError::UnknownSchema { found: 2, .. }),
		"{error:?}"
	);
}

#[test]
fn an_unreadable_schema_is_refused() {
	let tree = Tree::new(&[(REPOSITORY, BASE), ("docs/source-sentry.lock", "{")]);
	let error = migrate(&tree.snapshot, &tree.inventory).unwrap_err();
	assert!(
		matches!(error, MutateError::UnreadableSchema { .. }),
		"{error:?}"
	);
}
