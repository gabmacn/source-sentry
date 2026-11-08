//! Checks that span files: declarations, imports and paired configs.

use super::{
	BASE, REPOSITORY, directory_error, errors, load, only_error, repository_error,
};
use crate::config::{
	ConfigErrorKind, ConfigKind, DEFAULT_CONTEXT, IMPLICIT_VERSION,
};
use crate::testing::path;

#[test]
fn an_assigned_class_must_be_declared() {
	let error = directory_error("classes: ['internal-docs', 'secret']\n");
	assert_eq!(
		error.kind,
		ConfigErrorKind::UndeclaredClass {
			class: "secret".to_owned()
		}
	);
	assert_eq!(error.setting.as_deref(), Some("classes[1]"));
	assert_eq!(
		error.correction.as_deref(),
		Some(
			"Declare it under `define.classes` in .source-sentry/source-sentry.yaml"
		)
	);
}

#[test]
fn a_class_used_in_a_rule_must_be_declared() {
	let added = directory_error(
		"rules:\n  production:\n    - classes: ['secret']\n      facts: ['stale']\n      \
		 outcome: fail\n",
	);
	assert_eq!(
		added.kind,
		ConfigErrorKind::UndeclaredClass {
			class: "secret".to_owned()
		}
	);
	assert_eq!(
		added.setting.as_deref(),
		Some("rules.production[0].classes")
	);
	let defined = only_error(&[(
		REPOSITORY,
		"version: 1\ndefine:\n  contexts:\n    staging:\n      fallback: warn\n      rules:\n        \
		 - classes: ['secret']\n          facts: ['stale']\n          outcome: fail\n",
	)]);
	assert_eq!(
		defined.setting.as_deref(),
		Some("define.contexts.staging.rules[0].classes")
	);
}

#[test]
fn a_rule_context_must_be_declared() {
	let error = directory_error(
		"rules:\n  prodution:\n    - facts: ['stale']\n      outcome: fail\n",
	);
	assert_eq!(
		error.kind,
		ConfigErrorKind::UndeclaredContext {
			context: "prodution".to_owned()
		}
	);
	assert_eq!(error.setting.as_deref(), Some("rules.prodution"));
	// The built-in default context is always declared.
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"rules:\n  default:\n    - facts: ['unattested']\n      outcome: notice\n",
		),
	]);
	assert!(configs.is_declared_context(DEFAULT_CONTEXT));
	assert!(configs.is_declared_context("production"));
	assert!(!configs.is_declared_context("staging"));
	assert!(!configs.contexts().contains_key(DEFAULT_CONTEXT));
}

#[test]
fn no_rule_may_target_fresh() {
	let added = directory_error(
		"rules:\n  production:\n    - facts: ['fresh', 'stale']\n      outcome: warn\n",
	);
	assert_eq!(added.kind, ConfigErrorKind::FreshRule);
	assert_eq!(added.setting.as_deref(), Some("rules.production[0].facts"));
	let repository = repository_error(
		"rules:\n  production:\n    - facts: ['fresh']\n      outcome: pass\n",
	);
	assert_eq!(repository.kind, ConfigErrorKind::FreshRule);
}

#[test]
fn no_context_rule_may_target_fresh() {
	let error = only_error(&[(
		REPOSITORY,
		"version: 1\ndefine:\n  contexts:\n    staging:\n      fallback: warn\n      rules:\n        \
		 - facts: ['fresh']\n          outcome: pass\n",
	)]);
	assert_eq!(error.kind, ConfigErrorKind::FreshRule);
	assert_eq!(
		error.setting.as_deref(),
		Some("define.contexts.staging.rules[0].facts")
	);
}

#[test]
fn define_appears_only_in_the_repository_config() {
	for (file, kind) in [
		("docs/source-sentry.yaml", ConfigKind::Directory),
		(".source-sentry/presets/shared.yaml", ConfigKind::Preset),
		(".source-sentry/remotes/legal.yaml", ConfigKind::Paired),
	] {
		let error = only_error(&[
			(REPOSITORY, BASE),
			(file, "define:\n  classes:\n    extra: {}\n"),
		]);
		assert_eq!(
			error.kind,
			ConfigErrorKind::ForbiddenSetting {
				key: "define".to_owned(),
				kind,
			}
		);
	}
}

#[test]
fn without_a_repository_config_only_built_ins_are_declared() {
	let configs = load(&[("docs/source-sentry.yaml", "rules:\n  default: []\n")]);
	assert_eq!(configs.version().value, IMPLICIT_VERSION);
	assert_eq!(configs.version().origin.file, None);
	assert_eq!(configs.time_zone().value, "UTC");
	assert!(configs.classes().is_empty());
	let error =
		only_error(&[("docs/source-sentry.yaml", "classes: ['privacy']\n")]);
	assert_eq!(
		error.correction.as_deref(),
		Some(
			"Create .source-sentry/source-sentry.yaml and declare it under \
			 `define.classes`"
		)
	);
}

#[test]
fn an_invalid_repository_config_does_not_cascade() {
	let found = errors(&[
		(REPOSITORY, "version: 1\ndefine: [oops]\n"),
		("docs/source-sentry.yaml", "classes: ['privacy']\n"),
	]);
	assert_eq!(found.len(), 1, "{found:#?}");
	assert_eq!(found[0].file, path(REPOSITORY));
}

#[test]
fn an_unknown_preset_is_an_error() {
	let error = only_error(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/logs.yaml", ""),
		("docs/source-sentry.yaml", "imports: ['logs', 'audits']\n"),
	]);
	assert_eq!(
		error.kind,
		ConfigErrorKind::UnknownPreset {
			name: "audits".to_owned()
		}
	);
	assert_eq!(error.setting.as_deref(), Some("imports[1]"));
	assert_eq!(error.correction.as_deref(), Some("The presets are 'logs'"));
	let misnamed = only_error(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/audits.yml", ""),
		("docs/source-sentry.yaml", "imports: ['audits']\n"),
	]);
	assert_eq!(
		misnamed.correction.as_deref(),
		Some("Rename .source-sentry/presets/audits.yml to end in `.yaml`")
	);
}

#[test]
fn import_names_stay_inside_the_presets_folder() {
	let extension = directory_error("imports: ['logs.yaml']\n");
	assert!(matches!(
		extension.kind,
		ConfigErrorKind::InvalidImport { .. }
	));
	assert_eq!(extension.correction.as_deref(), Some("Use 'logs'"));
	for name in ["../secrets", "/logs", "teams//legal", "teams\\\\legal", ""] {
		let error = only_error(&[
			(REPOSITORY, BASE),
			("docs/source-sentry.yaml", &format!("imports: ['{name}']\n")),
		]);
		assert!(
			matches!(error.kind, ConfigErrorKind::InvalidImport { .. }),
			"{name}: {error:#?}"
		);
	}
}

#[test]
fn presets_in_subfolders_are_imported_by_their_path() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/presets/teams/legal.yaml",
			"owners: ['legal']\n",
		),
		("docs/source-sentry.yaml", "imports: ['teams/legal']\n"),
	]);
	let owners = configs.settings(&path("docs")).defaults.owners;
	assert_eq!(
		owners.map(|owners| owners.value),
		Some(vec!["legal".to_owned()])
	);
}

#[test]
fn an_import_cycle_is_an_error() {
	let error = only_error(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/a.yaml", "imports: ['b']\n"),
		(".source-sentry/presets/b.yaml", "imports: ['c']\n"),
		(".source-sentry/presets/c.yaml", "imports: ['a']\n"),
		("docs/source-sentry.yaml", "imports: ['a']\n"),
	]);
	assert_eq!(
		error.kind,
		ConfigErrorKind::ImportCycle {
			cycle: vec![
				"a".to_owned(),
				"b".to_owned(),
				"c".to_owned(),
				"a".to_owned()
			]
		}
	);
	assert_eq!(error.file, path(".source-sentry/presets/c.yaml"));
	let itself = only_error(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/a.yaml", "imports: ['a']\n"),
	]);
	assert_eq!(
		itself.kind,
		ConfigErrorKind::ImportCycle {
			cycle: vec!["a".to_owned(), "a".to_owned()]
		}
	);
}

#[test]
fn a_repeated_import_is_not_a_cycle() {
	load(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/common.yaml", ""),
		(
			".source-sentry/presets/a.yaml",
			"imports: ['common', 'common']\n",
		),
		("docs/source-sentry.yaml", "imports: ['a', 'common', 'a']\n"),
	]);
}

#[test]
fn a_broken_preset_is_not_also_unknown() {
	let found = errors(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/logs.yaml", "root: true\n"),
		("docs/source-sentry.yaml", "imports: ['logs']\n"),
	]);
	assert_eq!(found.len(), 1, "{found:#?}");
	assert_eq!(found[0].file, path(".source-sentry/presets/logs.yaml"));
}

#[test]
fn a_paired_config_cannot_import_forbidden_settings() {
	let error = only_error(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/presets/logs.yaml",
			"documents:\n  ignore: ['tmp/']\n",
		),
		(".source-sentry/remotes/legal.yaml", "imports: ['logs']\n"),
	]);
	assert_eq!(
		error.kind,
		ConfigErrorKind::ForbiddenSetting {
			key: "documents".to_owned(),
			kind: ConfigKind::Paired,
		}
	);
	assert_eq!(error.file, path(".source-sentry/presets/logs.yaml"));
	assert_eq!(
		error.import_chain,
		[path(".source-sentry/remotes/legal.yaml")]
	);
	// The same preset is fine for a directory.
	load(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/presets/logs.yaml",
			"documents:\n  ignore: ['tmp/']\n",
		),
		("docs/source-sentry.yaml", "imports: ['logs']\n"),
	]);
}

#[test]
fn a_paired_config_cannot_import_rules_through_a_nested_preset() {
	let found = errors(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/presets/strict.yaml",
			"rules:\n  production: []\nseries: {}\n",
		),
		(
			".source-sentry/presets/legal.yaml",
			"imports: ['strict']\nowners: ['legal']\n",
		),
		(".source-sentry/remotes/legal.yaml", "imports: ['legal']\n"),
	]);
	let keys: Vec<_> = found
		.iter()
		.filter_map(|error| error.setting.as_deref())
		.collect();
	assert_eq!(keys, ["rules", "series"]);
	assert_eq!(
		found[0].import_chain,
		[
			path(".source-sentry/remotes/legal.yaml"),
			path(".source-sentry/presets/legal.yaml")
		]
	);
	assert!(found[0].to_string().contains(
		"(imported through .source-sentry/remotes/legal.yaml → \
		 .source-sentry/presets/legal.yaml)"
	));
}
