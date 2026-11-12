//! Sharing declarations and settings through explicit project imports.

use super::{ConfigErrorKind, ConfigSet, LoadError, snapshot};
use crate::testing::path;

/// The closest available preset wins, without inheriting enclosing configs.
#[test]
fn preset_lookup_walks_projects_outwards_and_shadows_by_name() {
	for (own, parent, expected) in
		[(true, true, 2), (false, true, 3), (false, false, 4)]
	{
		let mut files = vec![
			(".source-sentry/source-sentry.yaml", "invalid: true\n"),
			(
				".source-sentry/presets/shared.yaml",
				"schedule: {grace: 4}\n",
			),
			(".source-sentry/presets/unused.yaml", "invalid: true\n"),
			(
				"app/.source-sentry/source-sentry.yaml",
				"version: 1\nschedule: {grace: 99}\n",
			),
			(
				"app/child/.source-sentry/source-sentry.yaml",
				"version: 1\nimports: [shared]\n",
			),
		];
		if parent {
			files.push((
				"app/.source-sentry/presets/shared.yaml",
				"schedule: {grace: 3}\n",
			));
		}
		if own {
			files.push((
				"app/child/.source-sentry/presets/shared.yaml",
				"schedule: {grace: 2}\n",
			));
		}
		let configs =
			ConfigSet::load_project(&snapshot(&files), &path("app/child")).unwrap();
		assert_eq!(
			configs
				.settings(&path("app/child"))
				.defaults
				.grace
				.unwrap()
				.value,
			expected
		);
	}
}

/// A project imports definitions explicitly and their origins retain the chain.
#[test]
fn projects_import_class_and_context_definitions() {
	let configs = ConfigSet::load_project(&snapshot(&[
		(".source-sentry/presets/shared.yaml", "define:\n  classes:\n    legal: {}\n  contexts:\n    production:\n      fallback: fail\nclasses: [legal]\n"),
		("app/.source-sentry/source-sentry.yaml", "version: 1\nimports: [shared]\n"),
	]), &path("app")).unwrap();
	assert!(configs.is_declared_context("production"));
	let class = configs.classes().values().next().unwrap();
	assert_eq!(
		class.origin.file,
		Some(path(".source-sentry/presets/shared.yaml"))
	);
	assert_eq!(
		class.origin.import_chain,
		[path("app/.source-sentry/source-sentry.yaml")]
	);
}

/// Duplicate explicit class and context definitions name both files.
#[test]
fn duplicate_definitions_are_errors_instead_of_overrides() {
	for definitions in [
		"classes:\n    legal: {}",
		"contexts:\n    production:\n      fallback: warn",
	] {
		for second in [
			"version: 1\nimports: [first]\n".to_owned()
				+ "define:\n  "
				+ definitions
				+ "\n",
			"version: 1\nimports: [first, second]\n".to_owned(),
		] {
			let definition = format!("define:\n  {definitions}\n");
			let snapshot = snapshot(&[
				(".source-sentry/presets/first.yaml", &definition),
				(".source-sentry/presets/second.yaml", &definition),
				("app/.source-sentry/source-sentry.yaml", &second),
			]);
			let Err(LoadError::Invalid(errors)) =
				ConfigSet::load_project(&snapshot, &path("app"))
			else {
				panic!("expected duplicate definitions");
			};
			let duplicate = errors
				.iter()
				.find(|error| {
					matches!(error.kind, ConfigErrorKind::DuplicateDefinition { .. })
				})
				.unwrap();
			assert!(
				duplicate
					.to_string()
					.contains(".source-sentry/presets/first.yaml")
			);
			assert!(duplicate.to_string().contains(&duplicate.file.to_string()));
		}
	}
}

/// Defining presets may only be imported directly by the project's own config.
#[test]
fn defining_presets_are_refused_from_directory_preset_and_remote_configs() {
	for (importer, settings) in [
		("app/docs/source-sentry.yaml", "imports: [definitions]\n"),
		(
			"app/.source-sentry/presets/outer.yaml",
			"imports: [definitions]\n",
		),
		(
			"app/.source-sentry/remotes/legal.yaml",
			"imports: [definitions]\n",
		),
	] {
		let snapshot = snapshot(&[
			(
				".source-sentry/presets/definitions.yaml",
				"define:\n  classes:\n    legal: {}\n",
			),
			("app/.source-sentry/source-sentry.yaml", "version: 1\n"),
			(importer, settings),
		]);
		let Err(LoadError::Invalid(errors)) =
			ConfigSet::load_project(&snapshot, &path("app"))
		else {
			panic!("expected an import restriction");
		};
		assert!(
			errors.iter().any(|error| matches!(
				error.kind,
				ConfigErrorKind::DefinitionImport { .. }
			) && error.file == path(importer)),
			"{errors:?}"
		);
	}
}
