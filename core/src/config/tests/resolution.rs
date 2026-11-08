//! Resolution order, imports, the cut, rules, series, remote owners and
//! origins.

use super::{BASE, REPOSITORY, load};
use crate::config::{
	ConfigSet, DirectorySettings, Origin, PathPattern, Position, Scope,
};
use crate::model::{Fact, Lifecycle, Outcome, RepoPath, VersionKind};
use crate::testing::path;

/// The owners `settings` resolve to.
fn owners(settings: &DirectorySettings) -> Vec<String> {
	settings
		.defaults
		.owners
		.as_ref()
		.map(|owners| owners.value.clone())
		.unwrap_or_default()
}

/// The ignore patterns `settings` accumulate, as written.
fn ignores(settings: &DirectorySettings) -> Vec<&str> {
	settings
		.ignore
		.iter()
		.map(|ignore| ignore.value.as_str())
		.collect()
}

/// Every layer of the resolution order sets `owners` and an ignore pattern.
fn layered() -> ConfigSet {
	load(&[
		(
			REPOSITORY,
			&format!(
				"{BASE}imports: ['repository-preset']\nowners: ['repository']\n\
				 schedule:\n  grace: 1\ndocuments:\n  ignore: ['repository']\n"
			),
		),
		(
			".source-sentry/presets/repository-preset.yaml",
			"owners: ['repository-preset']\ndocuments:\n  ignore: ['repository-preset']\n",
		),
		(
			"a/source-sentry.yaml",
			"owners: ['a']\nschedule:\n  grace: 2\ndocuments:\n  ignore: ['a']\n",
		),
		(
			".source-sentry/presets/import.yaml",
			"owners: ['import']\nschedule:\n  grace: 3\ndocuments:\n  ignore: ['import']\n",
		),
		(
			"a/b/source-sentry.yaml",
			"imports: ['import']\nowners: ['own']\ndocuments:\n  ignore: ['own']\n",
		),
	])
}

#[test]
fn settings_resolve_in_the_full_order() {
	let configs = layered();
	let own = configs.settings(&path("a/b"));
	assert_eq!(owners(&own), ["own"]);
	assert_eq!(
		ignores(&own),
		["repository-preset", "repository", "a", "import", "own"]
	);
	let grace = own.defaults.grace.as_ref().map(|grace| grace.value);
	assert_eq!(grace, Some(3));

	let ancestor = configs.settings(&path("a"));
	assert_eq!(owners(&ancestor), ["a"]);
	assert_eq!(ancestor.defaults.grace.map(|grace| grace.value), Some(2));

	let elsewhere = configs.settings(&path("elsewhere/deep"));
	assert_eq!(owners(&elsewhere), ["repository"]);
	assert_eq!(ignores(&elsewhere), ["repository-preset", "repository"]);

	// The built-in defaults come first, and nothing overrode `include`.
	assert_eq!(own.include.origin, Origin::built_in());
	let include: Vec<&str> =
		own.include.value.iter().map(PathPattern::as_str).collect();
	assert_eq!(include, ["*.md", "*.txt", "*.pdf"]);
}

#[test]
fn a_subdirectory_without_a_config_inherits_everything() {
	let configs = layered();
	assert_eq!(
		configs.settings(&path("a/b/c/d")),
		DirectorySettings {
			directory: path("a/b/c/d"),
			..configs.settings(&path("a/b"))
		}
	);
}

#[test]
fn an_empty_config_resets_nothing() {
	let configs = load(&[
		(REPOSITORY, &format!("{BASE}owners: ['repository']\n")),
		("a/source-sentry.yaml", ""),
		("a/b/source-sentry.yaml", "documents: {}\nschedule: {}\n"),
	]);
	assert_eq!(owners(&configs.settings(&path("a/b"))), ["repository"]);
}

#[test]
fn schedule_fields_inherit_one_by_one() {
	let configs = load(&[
		(
			REPOSITORY,
			&format!(
				"{BASE}schedule:\n  grace: 5\n  cadence:\n    dtstart: 2026-12-01\n    \
				 rrule: 'FREQ=MONTHLY'\n"
			),
		),
		("a/source-sentry.yaml", "schedule:\n  grace: 0\n"),
	]);
	let defaults = configs.settings(&path("a")).defaults;
	assert_eq!(defaults.grace.as_ref().map(|grace| grace.value), Some(0));
	let cadence = defaults.cadence.expect("the cadence is inherited");
	let rule = cadence.value.as_ref().map(|cadence| cadence.rrule.as_str());
	assert_eq!(rule, Some("FREQ=MONTHLY"));
	assert_eq!(cadence.origin.file, Some(path(REPOSITORY)));
}

/// `cadence: null` removes an inherited cadence and keeps the inherited grace,
/// without cutting any other setting.
#[test]
fn cadence_null_removes_an_inherited_cadence() {
	let repository = format!(
		"{BASE}schedule:\n  grace: 5\n  cadence:\n    dtstart: 2026-12-01\n    rrule: 'FREQ=MONTHLY'\n"
	);
	let configs = load(&[
		(REPOSITORY, &repository),
		("a/source-sentry.yaml", "schedule:\n  cadence: null\n"),
	]);
	let defaults = configs.settings(&path("a")).defaults;
	assert_eq!(defaults.grace.as_ref().map(|grace| grace.value), Some(5));
	let cadence = defaults.cadence.expect("the removal is recorded");
	assert_eq!(cadence.value, None);
	assert_eq!(cadence.origin.file, Some(path("a/source-sentry.yaml")));
}

#[test]
fn imports_expand_depth_first_with_repetitions() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(
			".source-sentry/presets/x.yaml",
			"imports: ['z']\nowners: ['x']\ndocuments:\n  ignore: ['x']\n",
		),
		(
			".source-sentry/presets/y.yaml",
			"owners: ['y']\ndocuments:\n  ignore: ['y']\n",
		),
		(
			".source-sentry/presets/z.yaml",
			"owners: ['z']\ndocuments:\n  ignore: ['z']\n",
		),
		("docs/source-sentry.yaml", "imports: ['x', 'y', 'x']\n"),
	]);
	let docs = configs.settings(&path("docs"));
	assert_eq!(ignores(&docs), ["z", "x", "y", "z", "x"]);
	// The last import listed wins, because the config sets no owners itself.
	assert_eq!(owners(&docs), ["x"]);
}

#[test]
fn an_origin_names_the_import_chain_and_the_consuming_scope() {
	let configs = load(&[
		(REPOSITORY, BASE),
		(".source-sentry/presets/outer.yaml", "imports: ['inner']\n"),
		(
			".source-sentry/presets/inner.yaml",
			"# Shared.\nschedule:\n  grace: 7\n",
		),
		("docs/source-sentry.yaml", "imports: ['outer']\n"),
	]);
	let grace = configs
		.settings(&path("docs/guides"))
		.defaults
		.grace
		.expect("an import sets grace");
	assert_eq!(grace.value, 7);
	assert_eq!(
		grace.origin,
		Origin {
			file: Some(path(".source-sentry/presets/inner.yaml")),
			position: Some(Position { line: 3, column: 3 }),
			import_chain: vec![
				path("docs/source-sentry.yaml"),
				path(".source-sentry/presets/outer.yaml"),
			],
			scope: Scope::Directory(path("docs")),
		}
	);
	assert_eq!(
		grace.origin.to_string(),
		".source-sentry/presets/inner.yaml:3:3, imported through \
		 docs/source-sentry.yaml → .source-sentry/presets/outer.yaml, for \
		 directory docs"
	);
	assert_eq!(
		configs.revision(),
		&crate::ports::Revision::WorkingTree { head: None }
	);
}

#[test]
fn inherit_false_cuts_ancestor_directory_configs() {
	let configs = load(&[
		(
			REPOSITORY,
			&format!(
				"{BASE}imports: ['base']\ndocuments:\n  ignore: ['repository']\n"
			),
		),
		(
			".source-sentry/presets/base.yaml",
			"documents:\n  ignore: ['base']\n",
		),
		(".source-sentry/presets/team.yaml", "owners: ['team']\n"),
		(
			"a/source-sentry.yaml",
			"imports: ['team']\ndocuments:\n  ignore: ['a']\nrules:\n  production:\n    \
			 - facts: ['stale']\n      outcome: fail\n",
		),
		(
			"a/b/source-sentry.yaml",
			"inherit: false\ndocuments:\n  ignore: ['b']\n",
		),
		("a/b/c/source-sentry.yaml", "documents:\n  ignore: ['c']\n"),
	]);
	let cut = configs.settings(&path("a/b"));
	assert_eq!(ignores(&cut), ["base", "repository", "b"]);
	assert_eq!(cut.defaults.owners, None);
	assert!(cut.rules.is_empty());

	// The cut applies to the cut directory's descendants too, while they
	// still inherit from the cut directory itself.
	let nested = configs.settings(&path("a/b/c"));
	assert_eq!(ignores(&nested), ["base", "repository", "b", "c"]);
	let deeper = configs.settings(&path("a/b/c/d"));
	assert_eq!(ignores(&deeper), ["base", "repository", "b", "c"]);

	// The ancestor itself is unaffected.
	let ancestor = configs.settings(&path("a"));
	assert_eq!(owners(&ancestor), ["team"]);
	assert_eq!(ancestor.rules["production"].len(), 1);
}

#[test]
fn inherit_is_not_inherited() {
	let configs = load(&[
		(REPOSITORY, BASE),
		("a/source-sentry.yaml", "owners: ['a']\n"),
		("a/b/source-sentry.yaml", "inherit: false\n"),
		("a/b/c/source-sentry.yaml", "inherit: true\n"),
	]);
	// `a/b`'s cut still applies below it; `inherit: true` cannot undo it.
	assert_eq!(configs.settings(&path("a/b/c")).defaults.owners, None);
}

#[test]
fn rules_accumulate_with_every_contributor() {
	let rule = |outcome: &str| {
		format!(
			"rules:\n  production:\n    - facts: ['stale']\n      outcome: {outcome}\n"
		)
	};
	let configs = load(&[
		(REPOSITORY, &format!("{BASE}{}", rule("notice"))),
		(".source-sentry/presets/strict.yaml", &rule("fail")),
		(
			"docs/source-sentry.yaml",
			&format!("imports: ['strict']\n{}", rule("warn")),
		),
	]);
	let docs = configs.settings(&path("docs/guides"));
	let production = &docs.rules["production"];
	let outcomes: Vec<Outcome> =
		production.iter().map(|rule| rule.value.outcome).collect();
	assert_eq!(outcomes, [Outcome::Notice, Outcome::Fail, Outcome::Warn]);
	let files: Vec<_> = production
		.iter()
		.map(|rule| rule.origin.file.clone().expect("a file adds the rule"))
		.collect();
	assert_eq!(
		files,
		[
			path(REPOSITORY),
			path(".source-sentry/presets/strict.yaml"),
			path("docs/source-sentry.yaml"),
		]
	);
	assert_eq!(production[1].origin.scope, Scope::Directory(path("docs")));
	assert_eq!(production[0].value.facts, [Fact::Stale]);
	// A sibling directory sees only the repository's rule.
	assert_eq!(configs.settings(&path("src")).rules["production"].len(), 1);
}

#[test]
fn series_belong_to_the_scope_that_defines_or_imports_them() {
	let series = |name: &str, pattern: &str| {
		format!(
			"series:\n  {name}:\n    pattern: '{pattern}'\n    version-kind: integer\n    \
			 lifecycle: records-only\n"
		)
	};
	let configs = load(&[
		(
			REPOSITORY,
			&format!("{BASE}{}", series("releases", r"v(?P<version>\d+)\.md")),
		),
		(
			".source-sentry/presets/audits.yaml",
			&series("audits", r"audit-(?P<version>\d+)\.md"),
		),
		("audits/source-sentry.yaml", "imports: ['audits']\n"),
	]);
	let below = configs.settings(&path("audits/2026"));
	let names: Vec<(&str, &Scope)> = below
		.series
		.iter()
		.map(|series| (series.name.as_str(), &series.scope))
		.collect();
	assert_eq!(
		names,
		[
			("releases", &Scope::Repository),
			("audits", &Scope::Directory(path("audits"))),
		]
	);
	let audits = &below.series[1];
	assert_eq!(audits.anchor, path("audits"));
	assert_eq!(audits.version_kind, VersionKind::Integer);
	assert_eq!(audits.lifecycle, Lifecycle::RecordsOnly);
	assert_eq!(
		audits.version_of(&path("audits/audit-0007.md")),
		Some("0007")
	);
	// Matched relative to the scope, and anchored at both ends.
	assert_eq!(audits.version_of(&path("audits/2026/audit-0007.md")), None);
	assert_eq!(audits.version_of(&path("audits/old-audit-0007.md")), None);
	assert_eq!(audits.version_of(&path("audits/audit-0007.md.bak")), None);
	assert_eq!(audits.version_of(&path("audit-0007.md")), None);
	let entry = path("audits/audit-0001.md");
	let matches = below.series_of(&entry);
	assert_eq!(matches.len(), 1);
	assert_eq!(matches[0].1, "0001");
}

#[test]
fn a_later_series_definition_replaces_one_of_the_same_scope() {
	let series = |pattern: &str| {
		format!(
			"series:\n  logs:\n    pattern: '{pattern}'\n    version-kind: integer\n    \
			 lifecycle: living\n"
		)
	};
	let configs = load(&[
		(
			REPOSITORY,
			&format!("{BASE}{}", series(r"(?P<version>\d+)\.md")),
		),
		(
			".source-sentry/presets/logs.yaml",
			&series(r"log-(?P<version>\d+)\.md"),
		),
		(
			"logs/source-sentry.yaml",
			&format!(
				"imports: ['logs', 'logs']\n{}",
				series(r"entry-(?P<version>\d+)\.md")
			),
		),
	]);
	let logs = configs.settings(&path("logs"));
	let patterns: Vec<(&Scope, &str)> = logs
		.series
		.iter()
		.map(|series| (&series.scope, series.pattern.as_str()))
		.collect();
	assert_eq!(
		patterns,
		[
			(&Scope::Repository, r"(?P<version>\d+)\.md"),
			(
				&Scope::Directory(path("logs")),
				r"entry-(?P<version>\d+)\.md"
			),
		]
	);
}

#[test]
fn remote_owners_resolve_after_the_repository_config() {
	let configs = load(&[
		(
			REPOSITORY,
			&format!(
				"{BASE}owners: ['repository']\nclasses: ['internal-docs']\nschedule:\n  grace: 1\n\
				 rules:\n  production:\n    - facts: ['stale']\n      outcome: fail\n"
			),
		),
		(
			".source-sentry/presets/legal.yaml",
			"classes: ['privacy']\nschedule:\n  grace: 14\n",
		),
		(
			".source-sentry/remotes/legal.yaml",
			"imports: ['legal']\nowners: ['legal']\nschedule:\n  grace: 30\n",
		),
		// A directory config never reaches remote documents.
		("source-sentry.yaml", "owners: ['root-directory']\n"),
	]);
	let legal = configs.remote_settings("legal");
	assert_eq!(legal.owner, "legal");
	let owners = legal.defaults.owners.expect("owners are set");
	assert_eq!(owners.value, ["legal"]);
	assert_eq!(owners.origin.scope, Scope::Remote("legal".to_owned()));
	let classes = legal.defaults.classes.expect("classes are set");
	assert_eq!(classes.value[0].as_str(), "privacy");
	assert_eq!(
		classes.origin.import_chain,
		[path(".source-sentry/remotes/legal.yaml")]
	);
	assert_eq!(legal.defaults.grace.map(|grace| grace.value), Some(30));
	assert_eq!(legal.rules["production"].len(), 1);

	// Without a paired config, the repository's defaults apply.
	let marketing = configs.remote_settings("marketing");
	assert_eq!(
		marketing.defaults.owners.map(|owners| owners.value),
		Some(vec!["repository".to_owned()])
	);
	assert_eq!(marketing.defaults.grace.map(|grace| grace.value), Some(1));
}

#[test]
fn directory_configs_are_found_whatever_the_ignores_say() {
	let configs = load(&[
		(
			REPOSITORY,
			&format!("{BASE}documents:\n  ignore: ['vendor/']\n"),
		),
		("vendor/source-sentry.yaml", "owners: ['vendor']\n"),
	]);
	assert_eq!(owners(&configs.settings(&path("vendor"))), ["vendor"]);
	assert_eq!(
		configs.config_directories().collect::<Vec<&RepoPath>>(),
		[&path("vendor")]
	);
}
