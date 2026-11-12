//! Each kind of config with its allowed and forbidden keys, and the strict YAML
//! hazards.

use super::{
	BASE, REPOSITORY, assert_forbidden, directory_error, errors, load,
	only_error, repository_error,
};
use crate::config::{ConfigErrorKind, ConfigKind, Position};
use crate::testing::path;

#[test]
fn the_approved_examples_load() {
	let configs = load(&[
		(
			REPOSITORY,
			include_str!("../../../../../structure/repository-config.example.yaml"),
		),
		(".source-sentry/presets/engineering.yaml", ""),
		(".source-sentry/presets/common.yaml", ""),
		(
			".source-sentry/presets/logs.yaml",
			include_str!("../../../../../structure/preset.example.yaml"),
		),
		(
			"docs/source-sentry.yaml",
			include_str!("../../../../../structure/directory-config.example.yaml"),
		),
		(
			".source-sentry/remotes/legal.yaml",
			include_str!("../../../../../structure/remote-config.example.yaml"),
		),
	]);
	assert_eq!(configs.version().value, 1);
	assert_eq!(configs.time_zone().value, "UTC");
	assert_eq!(configs.classes().len(), 4);
	assert!(configs.contexts().contains_key("production"));
	let docs = configs.settings(&path("docs"));
	assert_eq!(docs.series.len(), 2);
	let legal = configs.remote_settings("legal");
	assert_eq!(legal.defaults.grace.map(|grace| grace.value), Some(14));
}

#[test]
fn a_repository_config_allows_its_own_keys() {
	let configs = load(&[(
		REPOSITORY,
		&format!(
			"{BASE}time-zone: 'Europe/Madrid'\nimports: []\nclasses: ['privacy']\n\
			 owners: ['legal']\nschedule:\n  grace: 3\ndocuments:\n  include: ['*.md']\n\
			 rules:\n  production: []\nseries: {{}}\n"
		),
	)]);
	assert_eq!(configs.time_zone().value, "Europe/Madrid");
	assert_eq!(configs.time_zone().origin.file, Some(path(REPOSITORY)));
}

#[test]
fn a_repository_config_rejects_root_and_inherit() {
	assert_forbidden(&repository_error("root: true\n"), "root");
	assert_forbidden(&repository_error("inherit: false\n"), "inherit");
}

#[test]
fn a_repository_config_must_pin_a_supported_version() {
	let missing = only_error(&[(REPOSITORY, "time-zone: 'UTC'\n")]);
	assert_eq!(
		missing.kind,
		ConfigErrorKind::MissingSetting {
			key: "version".to_owned()
		}
	);
	let unsupported = only_error(&[(REPOSITORY, "version: 2\n")]);
	assert_eq!(
		unsupported.kind,
		ConfigErrorKind::UnsupportedVersion { version: 2 }
	);
	assert_eq!(unsupported.correction.as_deref(), Some("Use `version: 1`"));
}

#[test]
fn a_directory_config_allows_root_and_inherit() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"root: false\ninherit: false\nclasses: ['privacy']\n",
		),
	]);
	assert!(!configs.is_root(&path("docs")));
}

#[test]
fn a_directory_config_rejects_repository_keys() {
	assert_forbidden(&directory_error("version: 1\n"), "version");
	assert_forbidden(&directory_error("time-zone: 'UTC'\n"), "time-zone");
	assert_forbidden(
		&directory_error("define:\n  classes:\n    extra: {}\n"),
		"define",
	);
}

#[test]
fn a_preset_rejects_root_inherit_version_and_define() {
	for (key, value) in [
		("root", "true"),
		("inherit", "false"),
		("version", "1"),
		("time-zone", "'UTC'"),
		("define", "{}"),
	] {
		let error = only_error(&[
			(REPOSITORY, BASE),
			(
				".source-sentry/presets/shared.yaml",
				&format!("{key}: {value}\n"),
			),
		]);
		assert_forbidden(&error, key);
		assert_eq!(error.file, path(".source-sentry/presets/shared.yaml"));
	}
}

#[test]
fn a_paired_config_allows_only_four_keys() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/shared.yaml", "owners: ['legal']\n"),
		(
			".source-sentry/remotes/legal.yaml",
			"imports: ['shared']\nclasses: ['privacy']\nowners: ['legal']\n\
			 schedule:\n  grace: 14\n",
		),
	]);
	assert_eq!(configs.paired_owners().collect::<Vec<_>>(), ["legal"]);
	for (key, value) in [
		("documents", "{}"),
		("rules", "{}"),
		("series", "{}"),
		("root", "true"),
		("inherit", "false"),
		("version", "1"),
		("define", "{}"),
	] {
		let error = only_error(&[
			(REPOSITORY, BASE),
			(
				".source-sentry/remotes/legal.yaml",
				&format!("{key}: {value}\n"),
			),
		]);
		assert_forbidden(&error, key);
	}
}

#[test]
fn an_unknown_key_lists_the_keys_this_kind_allows() {
	let error = directory_error("ignore: ['scratch/']\n");
	assert_eq!(
		error.kind,
		ConfigErrorKind::UnknownSetting {
			key: "ignore".to_owned()
		}
	);
	assert_eq!(error.setting.as_deref(), Some("ignore"));
	assert_eq!(
		error.correction.as_deref(),
		Some(
			"A directory config allows `root`, `inherit`, `imports`, `classes`, \
			 `owners`, `schedule`, `documents`, `rules` or `series`"
		)
	);
	let nested = directory_error("documents:\n  includes: ['*.md']\n");
	assert_eq!(nested.setting.as_deref(), Some("documents.includes"));
	assert_eq!(
		nested.correction.as_deref(),
		Some("`documents` allows `include`, `ignore` or `adapters`")
	);
	let context = only_error(&[(
		REPOSITORY,
		"version: 1\ndefine:\n  contexts:\n    staging:\n      fallback: warn\n      \
		 rule: []\n",
	)]);
	assert_eq!(
		context.setting.as_deref(),
		Some("define.contexts.staging.rule")
	);
	assert_eq!(
		context.correction.as_deref(),
		Some("`define.contexts.staging` allows `fallback` or `rules`")
	);
}

#[test]
fn an_unquoted_glob_is_an_alias() {
	let error = directory_error("documents:\n  include: [*.md]\n");
	assert_eq!(
		error.kind,
		ConfigErrorKind::UnquotedAlias {
			token: "*.md".to_owned()
		}
	);
	assert_eq!(error.correction.as_deref(), Some("Quote it: use '*.md'"));
	assert_eq!(error.setting.as_deref(), Some("documents.include"));
	assert_eq!(
		error.position,
		Some(Position {
			line: 2,
			column: 13
		})
	);
	let block = directory_error("documents:\n  ignore:\n    - *.log\n");
	assert_eq!(block.correction.as_deref(), Some("Quote it: use '*.log'"));
}

#[test]
fn an_unquoted_exception_is_a_tag() {
	let error =
		directory_error("documents:\n  ignore:\n    - !drafts/ retained\n");
	assert_eq!(
		error.kind,
		ConfigErrorKind::UnsupportedTag {
			tag: "!drafts/".to_owned()
		}
	);
	assert_eq!(
		error.correction.as_deref(),
		Some("If the `!` belongs to the value, quote it: use '!drafts/ retained'")
	);
	let bare = directory_error("documents:\n  ignore: [!scratch/keep.md]\n");
	assert_eq!(
		bare.correction.as_deref(),
		Some("If the `!` belongs to the value, quote it: use '!scratch/keep.md'")
	);
}

#[test]
fn yaml_1_1_booleans_are_rejected() {
	let error = directory_error("inherit: no\n");
	assert_eq!(
		error.kind,
		ConfigErrorKind::NonStrictBoolean {
			value: "no".to_owned()
		}
	);
	assert_eq!(error.setting.as_deref(), Some("inherit"));
	assert_eq!(error.correction.as_deref(), Some("Use `false`"));
	let yes = directory_error("root: yes\n");
	assert_eq!(yes.correction.as_deref(), Some("Use `true`"));
}

#[test]
fn a_version_is_a_whole_number() {
	for written in ["1.10", "'1'", "-1"] {
		let error = only_error(&[(REPOSITORY, &format!("version: {written}\n"))]);
		assert!(
			matches!(&error.kind, ConfigErrorKind::NotWholeNumber { .. }),
			"{written}: {error:#?}"
		);
		assert_eq!(error.setting.as_deref(), Some("version"));
	}
	let float = only_error(&[(REPOSITORY, "version: 1.10\n")]);
	assert_eq!(
		float.kind,
		ConfigErrorKind::NotWholeNumber {
			value: "1.10".to_owned()
		}
	);
	let grace = directory_error("schedule:\n  grace: 1.5\n");
	assert_eq!(grace.setting.as_deref(), Some("schedule.grace"));
	assert_eq!(
		grace.kind,
		ConfigErrorKind::NotWholeNumber {
			value: "1.5".to_owned()
		}
	);
}

#[test]
fn duplicate_keys_merge_keys_and_documents_are_rejected() {
	let duplicate = directory_error("owners: ['a']\nowners: ['b']\n");
	assert_eq!(
		duplicate.kind,
		ConfigErrorKind::DuplicateKey {
			key: "owners".to_owned()
		}
	);
	let merge = directory_error(
		"schedule: &shared\n  grace: 1\ndocuments:\n  <<: *shared\n",
	);
	assert_eq!(merge.kind, ConfigErrorKind::MergeKey);
	let documents = directory_error("owners: ['a']\n---\nowners: ['b']\n");
	assert_eq!(documents.kind, ConfigErrorKind::MultipleDocuments);
}

#[test]
fn quoting_avoids_every_hazard() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"inherit: false\ndocuments:\n  include: ['*.md']\n  ignore:\n    - 'scratch/'\n    \
			 - '!scratch/keep.md'\n    - |-\n      *.log\n",
		),
	]);
	let docs = configs.settings(&path("docs"));
	let ignores: Vec<&str> = docs
		.ignore
		.iter()
		.map(|ignore| ignore.value.as_str())
		.collect();
	assert_eq!(ignores, ["scratch/", "!scratch/keep.md", "*.log"]);
}

#[test]
fn an_empty_value_is_rejected_but_an_empty_file_is_not() {
	let error = directory_error("classes:\n");
	assert_eq!(error.kind, ConfigErrorKind::EmptyValue);
	assert_eq!(error.setting.as_deref(), Some("classes"));
	let nested = directory_error("documents:\n  ignore: ~\n");
	assert_eq!(nested.setting.as_deref(), Some("documents.ignore"));
	load(&[(REPOSITORY, BASE), ("docs/source-sentry.yaml", "")]);
	load(&[
		(REPOSITORY, BASE),
		("docs/source-sentry.yaml", "# Nothing yet.\n"),
	]);
}

#[test]
fn errors_inside_model_types_name_the_full_setting() {
	let error = directory_error(
		"rules:\n  production:\n    - facts: ['stale', 'bogus']\n      outcome: warn\n",
	);
	assert_eq!(
		error.setting.as_deref(),
		Some("rules.production[0].facts[1]")
	);
	let context = repository_error(
		"rules:\n  production:\n    - facts: ['stale']\n      outcome: block\n",
	);
	assert_eq!(
		context.setting.as_deref(),
		Some("rules.production[0].outcome")
	);
	assert_eq!(
		context.correction.as_deref(),
		Some("Use `pass`, `notice`, `warn` or `fail`")
	);
	let adapter = directory_error(
		"documents:\n  adapters:\n    - match: '*.rst'\n      adapter: markdown\n",
	);
	assert_eq!(
		adapter.setting.as_deref(),
		Some("documents.adapters[0].adapter")
	);
	assert_eq!(
		adapter.correction.as_deref(),
		Some("Use `md-utf8` or `opaque`")
	);
}

#[test]
fn the_time_zone_is_a_non_empty_string() {
	let number = repository_error("time-zone: 1\n");
	assert_eq!(number.setting.as_deref(), Some("time-zone"));
	assert_eq!(
		number.correction.as_deref(),
		Some("Use an IANA name in quotes, such as 'Europe/Madrid'")
	);
	let empty = repository_error("time-zone: ''\n");
	assert_eq!(empty.kind, ConfigErrorKind::EmptyValue);
	// The zone itself is the schedule module's to validate.
	let configs =
		load(&[(REPOSITORY, &format!("{BASE}time-zone: 'Mars/Olympus'\n"))]);
	assert_eq!(configs.time_zone().value, "Mars/Olympus");
}

#[test]
fn a_file_that_is_not_utf8_is_rejected() {
	let snapshot = super::snapshot(&[(REPOSITORY, BASE)])
		.with_file("docs/source-sentry.yaml", &[0xff, 0xfe, b'a']);
	let Err(crate::config::LoadError::Invalid(errors)) =
		crate::config::ConfigSet::load(&snapshot)
	else {
		panic!("expected an error");
	};
	assert_eq!(errors.len(), 1);
	assert_eq!(errors[0].kind, ConfigErrorKind::NotUtf8);
}

#[test]
fn names_follow_kebab_case() {
	let context = repository_error("rules:\n  Production: []\n");
	assert_eq!(
		context.kind,
		ConfigErrorKind::InvalidName {
			name: "Production".to_owned(),
			what: "context name",
		}
	);
	assert_eq!(
		context.correction.as_deref(),
		Some("Use lowercase kebab-case, such as 'production'")
	);
	let class = directory_error("classes: ['Internal Docs']\n");
	assert_eq!(class.setting.as_deref(), Some("classes[0]"));
	assert_eq!(
		class.correction.as_deref(),
		Some("Use lowercase kebab-case, such as 'internal-docs'")
	);
	let series = directory_error(
		"series:\n  Daily_Logs:\n    pattern: '(?P<version>\\d+)\\.md'\n    version-kind: integer\n    \
		 lifecycle: living\n",
	);
	assert_eq!(series.setting.as_deref(), Some("series.Daily_Logs"));
	let owner = directory_error("owners: ['  ']\n");
	assert_eq!(owner.setting.as_deref(), Some("owners[0]"));
}

#[test]
fn path_patterns_are_checked_where_they_are_written() {
	let negated_include = directory_error("documents:\n  include: ['!*.md']\n");
	assert!(matches!(
		negated_include.kind,
		ConfigErrorKind::InvalidPattern { .. }
	));
	assert_eq!(
		negated_include.setting.as_deref(),
		Some("documents.include[0]")
	);
	assert_eq!(
		negated_include.correction.as_deref(),
		Some("To exclude these paths, add '*.md' to `documents.ignore`")
	);
	let negated_adapter = directory_error(
		"documents:\n  adapters:\n    - match: '!*.md'\n      adapter: opaque\n",
	);
	assert_eq!(
		negated_adapter.setting.as_deref(),
		Some("documents.adapters[0].match")
	);
	let comment = directory_error("documents:\n  ignore: ['#notes.md']\n");
	assert_eq!(
		comment.correction.as_deref(),
		Some("Use '\\#notes.md' to match a literal `#`")
	);
	let unclosed = directory_error("documents:\n  ignore: ['[ab']\n");
	assert!(matches!(
		unclosed.kind,
		ConfigErrorKind::InvalidPattern { .. }
	));
	let empty = directory_error("documents:\n  ignore: ['']\n");
	assert!(matches!(empty.kind, ConfigErrorKind::InvalidPattern { .. }));
	let missing =
		directory_error("documents:\n  adapters:\n    - adapter: opaque\n");
	assert_eq!(
		missing.kind,
		ConfigErrorKind::MissingSetting {
			key: "match".to_owned()
		}
	);
}

#[test]
fn series_patterns_need_a_version_group() {
	let series = |pattern: &str| {
		directory_error(&format!(
			"series:\n  logs:\n    pattern: '{pattern}'\n    version-kind: integer\n    \
			 lifecycle: records-only\n"
		))
	};
	let unnamed = series("log-(\\d+)\\.md");
	assert!(matches!(
		unnamed.kind,
		ConfigErrorKind::InvalidSeriesPattern { .. }
	));
	assert_eq!(unnamed.setting.as_deref(), Some("series.logs.pattern"));
	assert!(unnamed.correction.is_some());
	let invalid = series("log-(?P<version>\\d+");
	assert!(matches!(
		invalid.kind,
		ConfigErrorKind::InvalidSeriesPattern { .. }
	));
	let missing = directory_error(
		"series:\n  logs:\n    pattern: '(?P<version>\\d+)\\.md'\n    lifecycle: living\n",
	);
	assert_eq!(
		missing.kind,
		ConfigErrorKind::MissingSetting {
			key: "version-kind".to_owned()
		}
	);
	assert_eq!(missing.setting.as_deref(), Some("series.logs.version-kind"));
}

#[test]
fn every_error_in_a_file_is_reported() {
	let found = errors(&[
		(REPOSITORY, BASE),
		(
			"docs/source-sentry.yaml",
			"version: 1\nclasses:\nowners: ['  ']\ndocuments:\n  ignore: ['']\n",
		),
	]);
	let settings: Vec<_> = found
		.iter()
		.filter_map(|error| error.setting.as_deref())
		.collect();
	assert_eq!(
		settings,
		["version", "classes", "owners[0]", "documents.ignore[0]"]
	);
}

#[test]
fn an_error_renders_everything_needed_to_fix_it() {
	let error = directory_error("documents:\n  include: [*.md]\n");
	assert_eq!(
		error.to_string(),
		"docs/source-sentry.yaml:2:13: `documents.include`: YAML read `*.md` as an \
		 alias, because it starts with `*`. Quote it: use '*.md'"
	);
	assert_eq!(ConfigKind::Paired.to_string(), "paired config");
}
