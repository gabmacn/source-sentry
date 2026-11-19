//! Checks that span files: declarations, imports and paired configs.

use super::{
	BASE, REPOSITORY, directory_error, errors, load, only_error,
	repository_error, snapshot,
};
use crate::config::{
	ConfigErrorKind, ConfigKind, ConfigSet, DEFAULT_CONTEXT, IMPLICIT_VERSION,
	LoadError, MAX_EXPANDED_IMPORTS, MAX_IMPORT_DEPTH,
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
fn a_cadence_start_with_a_time_of_day_is_refused() {
	let error = directory_error(
		"schedule:
  cadence:
    dtstart: 2026-01-01T10:00:00
    rrule: 		 FREQ=DAILY
",
	);
	assert!(
		error
			.to_string()
			.contains("is not a date written YYYY-MM-DD"),
		"{error:?}"
	);
}

#[test]
fn an_invalid_cadence_is_reported_without_any_document() {
	let error = directory_error(
		"schedule:\n  grace: 5\n  cadence:\n    dtstart: 2026-01-01\n    rrule: \
		 'FREQ=NOTREAL'\n",
	);
	assert!(
		matches!(error.kind, ConfigErrorKind::InvalidSchedule { .. }),
		"{error:?}"
	);
	assert_eq!(error.setting.as_deref(), Some("schedule.cadence"));
	// The cadence's own line, not the `schedule` mapping's first, `grace`.
	assert_eq!(error.position.map(|position| position.line), Some(3));
}

#[test]
fn a_scoped_rule_needs_facts() {
	let added = directory_error(
		"rules:\n  production:\n    - facts: []\n      outcome: fail\n",
	);
	assert_eq!(added.kind, ConfigErrorKind::NoFacts);
	assert_eq!(added.setting.as_deref(), Some("rules.production[0].facts"));
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
fn directory_and_remote_configs_cannot_define_names() {
	for (file, kind) in [
		("docs/source-sentry.yaml", ConfigKind::Directory),
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
fn a_repeated_import_is_expanded_every_time_in_order() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/common.yaml", ""),
		(
			".source-sentry/presets/a.yaml",
			"imports: ['common', 'common']\n",
		),
		("docs/source-sentry.yaml", "imports: ['a', 'common', 'a']\n"),
	]);
	let docs = &configs.directories[&path("docs")];
	let expanded: Vec<(&str, usize)> = docs
		.imports
		.iter()
		.map(|import| {
			let name = import.preset.path.file_name().unwrap_or_default();
			(name, import.chain.len())
		})
		.collect();
	assert_eq!(
		expanded,
		[
			("common.yaml", 2),
			("common.yaml", 2),
			("a.yaml", 1),
			("common.yaml", 1),
			("common.yaml", 2),
			("common.yaml", 2),
			("a.yaml", 1),
		]
	);
}

/// Presets `p1` to `p{count}`, each importing the next one `times` times,
/// and a repository config that imports `p1`.
fn preset_chain(count: usize, times: usize) -> Vec<(String, String)> {
	let mut files =
		vec![(REPOSITORY.to_owned(), format!("{BASE}imports: ['p1']\n"))];
	for number in 1..=count {
		let imports = if number == count {
			String::new()
		} else {
			format!(
				"imports: [{}]\n",
				vec![format!("'p{}'", number + 1); times].join(", ")
			)
		};
		files.push((format!(".source-sentry/presets/p{number}.yaml"), imports));
	}
	files
}

/// Loads `files`, given as owned strings.
fn load_owned(files: &[(String, String)]) -> Result<ConfigSet, LoadError> {
	let files: Vec<(&str, &str)> = files
		.iter()
		.map(|(file, contents)| (file.as_str(), contents.as_str()))
		.collect();
	ConfigSet::load(&snapshot(&files))
}

#[test]
fn imports_nested_up_to_the_limit_load() {
	let files = preset_chain(MAX_IMPORT_DEPTH, 1);
	let Ok(configs) = load_owned(&files) else {
		panic!("a chain of {MAX_IMPORT_DEPTH} presets loads");
	};
	let repository = configs.repository.as_ref().expect("a repository config");
	assert_eq!(repository.imports.len(), MAX_IMPORT_DEPTH);
	assert_eq!(repository.imports[0].chain.len(), MAX_IMPORT_DEPTH);
}

#[test]
fn an_import_nested_past_the_limit_is_an_error() {
	let files = preset_chain(MAX_IMPORT_DEPTH + 1, 1);
	let Err(LoadError::Invalid(found)) = load_owned(&files) else {
		panic!("a chain one preset deeper than the limit is refused");
	};
	assert_eq!(found.len(), 1, "{found:#?}");
	let error = &found[0];
	assert_eq!(
		error.kind,
		ConfigErrorKind::ImportTooDeep {
			limit: MAX_IMPORT_DEPTH
		}
	);
	assert_eq!(
		error.file,
		path(&format!(".source-sentry/presets/p{MAX_IMPORT_DEPTH}.yaml"))
	);
	assert_eq!(error.setting.as_deref(), Some("imports[0]"));
	assert_eq!(error.import_chain.len(), MAX_IMPORT_DEPTH);
	assert_eq!(error.import_chain[0], path(REPOSITORY));
	assert!(
		error
			.to_string()
			.contains(&format!("the limit of {MAX_IMPORT_DEPTH} nested imports")),
		"{error}"
	);
}

#[test]
fn a_very_long_chain_of_presets_is_an_error_not_a_crash() {
	let files = preset_chain(2048, 1);
	let Err(LoadError::Invalid(found)) = load_owned(&files) else {
		panic!("a chain of 2048 presets is refused");
	};
	assert!(
		found.iter().all(|error| error.kind
			== ConfigErrorKind::ImportTooDeep {
				limit: MAX_IMPORT_DEPTH
			}),
		"{found:#?}"
	);
}

#[test]
fn imports_that_expand_past_the_limit_are_an_error() {
	// Each preset imports the next twice, so the expansion doubles with each
	// level, past the limit well before the chain ends.
	let levels = usize::try_from(MAX_EXPANDED_IMPORTS.ilog2()).unwrap_or(0) + 1;
	assert!(levels <= MAX_IMPORT_DEPTH);
	let files = preset_chain(levels, 2);
	let Err(LoadError::Invalid(found)) = load_owned(&files) else {
		panic!("an expansion past the limit is refused");
	};
	assert_eq!(found.len(), 1, "{found:#?}");
	let error = &found[0];
	assert_eq!(
		error.kind,
		ConfigErrorKind::TooManyImports {
			limit: MAX_EXPANDED_IMPORTS
		}
	);
	assert_eq!(error.import_chain[0], path(REPOSITORY));
	assert!(
		error.to_string().contains(&format!(
			"the limit of {MAX_EXPANDED_IMPORTS} expanded imports for one config"
		)),
		"{error}"
	);
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
