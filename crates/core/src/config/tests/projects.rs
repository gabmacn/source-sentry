//! Project ownership, configuration isolation and source roots.

use super::snapshot;
use crate::claims::{SourceGlob, discover};
use crate::config::{ConfigSet, project_of, project_roots};
use crate::model::{DocumentId, RepoPath, resolve_project_source};
use crate::testing::path;

/// Project ownership chooses the nearest tracked marker, by components.
#[test]
fn nearest_project_owns_each_path() {
	let roots = project_roots(&[
		path("app/.source-sentry/source-sentry.yaml"),
		path("app/child/.source-sentry/source-sentry.yaml"),
	]);
	for (file, root) in [
		("other/a.md", ""),
		("app/a.md", "app"),
		("app/child/a.md", "app/child"),
		("app-old/a.md", ""),
	] {
		assert_eq!(project_of(&roots, &path(file)), path(root));
	}
}

/// Configs, scans and series stop at project boundaries.
#[test]
fn projects_load_only_their_own_settings_and_claims() {
	let snapshot = snapshot(&[
		(
			".source-sentry/source-sentry.yaml",
			"version: 1\nclasses: []\nschedule: {grace: 9}\n",
		),
		(
			"source-sentry.lock",
			"{\"schema\":1,\"documents\":{},\"retired\":{}}",
		),
		("parent.md", "# Parent\n"),
		(
			"app/.source-sentry/source-sentry.yaml",
			"version: 1\nschedule: {grace: 2}\n",
		),
		(
			"app/source-sentry.lock",
			"{\"schema\":1,\"documents\":{\"a.md\":{\"sources\":[{\"path\":\"/code.rs\"},{\"path\":\":/code.rs\"}]}},\"retired\":{}}",
		),
		("app/a.md", "# App\n"),
		("app/code.rs", "app"),
		("code.rs", "parent"),
		(
			"app/nested/.source-sentry/source-sentry.yaml",
			"broken: true\n",
		),
		("app/nested/a.md", "# Nested\n"),
	]);
	let parent = ConfigSet::load(&snapshot).unwrap();
	let found = discover(&snapshot, &parent).unwrap();
	assert_eq!(found.unclaimed.len(), 1);
	assert_eq!(found.unclaimed[0].path, path("parent.md"));
	let app = ConfigSet::load_project(&snapshot, &path("app/a.md")).unwrap();
	assert_eq!(app.project(), &path("app"));
	assert_eq!(app.settings(&path("app")).defaults.grace.unwrap().value, 2);
	let found = discover(&snapshot, &app).unwrap();
	assert!(!found.has_errors(), "{:?}", found.diagnostics);
	assert_eq!(found.claims.len(), 1);
	assert_eq!(
		found.claims[0].document,
		DocumentId::Local(path("app/a.md"))
	);
	assert_eq!(
		found.claims[0].source_paths(),
		std::collections::BTreeSet::from([&path("app/code.rs"), &path("code.rs")])
	);
	assert_eq!(app.root_of(&path("app/nested/a.md")), None);
}

/// Both source roots and relative parents remain bounded by the repository.
#[test]
fn source_roots_and_globs_use_the_document_project() {
	let project = path("app");
	let document = path("app/docs/a.md");
	for (spelling, expected) in [
		("/src/a.rs", "app/src/a.rs"),
		(":/src/a.rs", "src/a.rs"),
		("../../shared/a.rs", "shared/a.rs"),
	] {
		assert_eq!(
			resolve_project_source(&project, Some(&document), spelling).unwrap(),
			path(expected)
		);
	}
	assert!(
		resolve_project_source(&project, Some(&document), ":/../escape").is_err()
	);
	assert!(resolve_project_source(&project, None, "relative").is_err());
	assert_eq!(
		SourceGlob::compile_project(&project, Some(&document), "/src/*.rs")
			.unwrap()
			.rooted(),
		"/app/src/*.rs"
	);
	assert_eq!(
		SourceGlob::compile_project(&project, None, ":/src/*.rs")
			.unwrap()
			.rooted(),
		"/src/*.rs"
	);
	assert_eq!(
		resolve_project_source(&RepoPath::ROOT, None, ":/src/a.rs").unwrap(),
		path("src/a.rs")
	);
}

/// The placement separator cannot be part of an owner loaded from disk either.
#[test]
fn remote_owner_filenames_cannot_contain_at_signs() {
	let paired =
		snapshot(&[(".source-sentry/remotes/team@other.yaml", "owners: [team]\n")]);
	assert!(
		matches!(ConfigSet::load(&paired), Err(crate::config::LoadError::Invalid(errors)) if errors.iter().any(|error| error.to_string().contains("cannot contain `@`")))
	);
	let remote = snapshot(&[(
		".source-sentry/remotes/team@other.lock",
		r#"{"schema":1,"documents":{},"retired":{}}"#,
	)]);
	let configs = ConfigSet::load(&remote).unwrap();
	let inventory = discover(&remote, &configs).unwrap();
	assert!(inventory.diagnostics.iter().any(|diagnostic| {
		diagnostic
			.message
			.contains("owner names cannot contain `@`")
	}));
}
