//! Source resolution, pattern expansion and dependencies between documents.

use std::collections::{BTreeMap, BTreeSet};

use super::{
	OID, REVIEW, assert_clean, inventory, kinds, local, lockfile, markdown, only,
	paths, project_inventory, sealed_with_source, with_source,
};
use crate::claims::sources::resolve;
use crate::claims::{
	Claim, Dependency, DiagnosticKind, Inventory, Mode, Presence,
	ResolvedPattern, ResolvedSource, Severity, SourceGlob,
};
use crate::model::{Declaration, PathSource, PatternSource, SourceDecl};
use crate::testing::path;

/// Both local and remote lockfiles are forbidden, even for sealed records;
/// diagnostics retain the source's written line and document.
#[test]
fn lockfile_path_sources_are_configuration_errors() {
	for source in [
		"/source-sentry.lock",
		"../source-sentry.lock",
		"/.source-sentry/remotes/legal.lock",
	] {
		let inventory = inventory(&[(
			"docs/a.md",
			&markdown(&format!("  sources:\n    - path: '{source}'\n")),
		)]);
		let diagnostic = only(&inventory, DiagnosticKind::LockfileSource);
		assert_eq!(diagnostic.severity(), Severity::Configuration);
		assert_eq!(diagnostic.document, Some(local("docs/a.md")));
		assert_eq!(diagnostic.location.line, Some(5));
	}
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", &sealed_with_source("source-sentry.lock"))], &[]),
		),
		("docs/a.md", ""),
	]);
	only(&inventory, DiagnosticKind::LockfileSource);
}

/// Lockfiles are excluded from current and recorded pattern membership;
/// configuration files remain ordinary sources.
#[test]
fn patterns_exclude_lockfiles_and_keep_configs() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["/inputs/**", "/.source-sentry/remotes/*"]),
		),
		("inputs/source-sentry.lock", &lockfile(&[], &[])),
		("inputs/source-sentry.yaml", "{}\n"),
		(".source-sentry/remotes/legal.lock", &lockfile(&[], &[])),
		(".source-sentry/remotes/legal.yaml", "{}\n"),
	]);
	assert_clean(&inventory);
	let found = patterns(&inventory, "docs/a.md");
	assert_eq!(matches(found[0]), ["inputs/source-sentry.yaml"]);
	assert_eq!(matches(found[1]), [".source-sentry/remotes/legal.yaml"]);
}

/// Old attestations of lockfiles do not remain pattern members, including
/// in sealed declarations, and a lockfile-only pattern is empty.
#[test]
fn recorded_pattern_members_also_exclude_lockfiles() {
	for sealed in [false, true] {
		let seal = if sealed {
			format!(
				r#", "seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {OID}}}"#
			)
		} else {
			String::new()
		};
		let declaration = format!(
			r#"{{"sources": [{{"pattern": "/**/source-sentry.lock", "members": [{{"path": "/source-sentry.lock", "oid": {OID}, "reviewed": {REVIEW}}}]}}]{seal}}}"#
		);
		let inventory = inventory(&[
			(
				"source-sentry.lock",
				&lockfile(&[("a.md", &declaration)], &[]),
			),
			("a.md", ""),
		]);
		let found = patterns(&inventory, "a.md");
		assert!(found[0].members.is_empty());
		assert_eq!(matches(found[0]), Vec::<String>::new());
		if sealed {
			assert_clean(&inventory);
		} else {
			only(&inventory, DiagnosticKind::EmptyPattern);
		}
	}
}

/// A `.lock` file in a `remotes` directory is an ordinary source unless
/// that directory is directly inside `.source-sentry/`.
#[test]
fn a_lock_file_outside_the_tool_directory_is_an_ordinary_source() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&markdown(concat!(
				"  sources:
",
				"    - path: '/archive/not.source-sentry/remotes/data.lock'
",
				"    - pattern: '/archive/not.source-sentry/remotes/*.lock'
",
			)),
		),
		(
			"archive/not.source-sentry/remotes/data.lock",
			"{}
",
		),
	]);
	assert_clean(&inventory);
	let claim = inventory.claims_of(&local("docs/a.md")).next().unwrap();
	assert_eq!(
		claim
			.source_paths()
			.iter()
			.map(ToString::to_string)
			.collect::<Vec<_>>(),
		["archive/not.source-sentry/remotes/data.lock"]
	);
	assert_eq!(
		matches(patterns(&inventory, "docs/a.md")[0]),
		["archive/not.source-sentry/remotes/data.lock"]
	);
}

/// The resolved patterns of the only claim of `document`.
fn patterns<'a>(
	inventory: &'a Inventory,
	document: &str,
) -> Vec<&'a ResolvedPattern> {
	let id = local(document);
	let claims: Vec<&Claim> = inventory.claims_of(&id).collect();
	let [claim] = claims.as_slice() else {
		panic!("expected one claim of {document}, got {claims:#?}");
	};
	claim
		.sources
		.iter()
		.filter_map(|source| match source {
			ResolvedSource::Pattern(pattern) => Some(pattern),
			ResolvedSource::Path(_) => None,
		})
		.collect()
}

/// A pattern's current matches, as sorted paths.
fn matches(pattern: &ResolvedPattern) -> Vec<String> {
	pattern
		.matches
		.iter()
		.flatten()
		.map(ToString::to_string)
		.collect()
}

/// A frontmatter declaration with one pattern source per entry of `patterns`.
fn with_patterns(patterns: &[&str]) -> String {
	let sources: Vec<String> = patterns
		.iter()
		.map(|pattern| format!("    - pattern: '{pattern}'\n"))
		.collect();
	markdown(&format!("  sources:\n{}", sources.concat()))
}

#[test]
fn a_single_star_stays_in_one_directory_and_a_double_star_descends() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["/src/*.rs", "/src/**/*.rs", "/src/**"]),
		),
		("src/lib.rs", ""),
		("src/deep/mod.rs", ""),
		("src/deep/er/x.rs", ""),
		("src/notes.txt", ""),
		("srcs/other.rs", ""),
	]);
	assert_clean(&inventory);
	let found: Vec<_> = patterns(&inventory, "docs/a.md")
		.into_iter()
		.map(|pattern| (pattern.glob.clone(), matches(pattern)))
		.collect();
	assert_eq!(
		found,
		[
			("/src/*.rs".to_owned(), vec!["src/lib.rs".to_owned()]),
			(
				"/src/**/*.rs".to_owned(),
				vec![
					"src/deep/er/x.rs".to_owned(),
					"src/deep/mod.rs".to_owned(),
					"src/lib.rs".to_owned()
				]
			),
			(
				"/src/**".to_owned(),
				vec![
					"src/deep/er/x.rs".to_owned(),
					"src/deep/mod.rs".to_owned(),
					"src/lib.rs".to_owned(),
					"src/notes.txt".to_owned()
				]
			),
		]
	);
}

#[test]
fn relative_and_rooted_patterns_resolve_from_their_bases() {
	let inventory = inventory(&[
		(
			"docs/guide/a.md",
			&with_patterns(&["*.txt", "./notes.*", "../api/*.md", "/*.md"]),
		),
		("docs/guide/notes.txt", ""),
		("docs/guide/deeper/notes.txt", ""),
		("docs/api/auth.md", ""),
		("README.md", ""),
		("notes.txt", ""),
	]);
	assert_clean(&inventory);
	let found: Vec<_> = patterns(&inventory, "docs/guide/a.md")
		.into_iter()
		.map(|pattern| (pattern.glob.clone(), matches(pattern)))
		.collect();
	assert_eq!(
		found,
		[
			(
				"/docs/guide/*.txt".to_owned(),
				vec!["docs/guide/notes.txt".to_owned()]
			),
			(
				"/docs/guide/notes.*".to_owned(),
				vec!["docs/guide/notes.txt".to_owned()]
			),
			(
				"/docs/api/*.md".to_owned(),
				vec!["docs/api/auth.md".to_owned()]
			),
			("/*.md".to_owned(), vec!["README.md".to_owned()]),
		]
	);
	// The union of current sources has no repeats, though two patterns match
	// the same file.
	let claim = inventory
		.claims_of(&local("docs/guide/a.md"))
		.next()
		.unwrap();
	assert_eq!(claim.source_paths().len(), 3);
}

/// Directories whose names an unescaped glob would read as glob syntax.
const GLOB_LIKE_DIRECTORIES: [&str; 5] =
	["[draft]", "star*", "q?", "{one,two}", "open["];

#[test]
fn a_documents_directory_is_literal_in_its_relative_patterns() {
	for directory in GLOB_LIKE_DIRECTORIES {
		let document = format!("docs/{directory}/guide.md");
		let own = format!("docs/{directory}/code.rs");
		let inventory = inventory(&[
			(&document, &with_patterns(&["*.rs", "../x/*.rs"])),
			(&own, ""),
			("docs/d/code.rs", ""),
			("docs/starX/code.rs", ""),
			("docs/qX/code.rs", ""),
			("docs/one/code.rs", ""),
			("docs/two/code.rs", ""),
			("docs/open[/other/code.rs", ""),
			("docs/x/a.rs", ""),
		]);
		assert_clean(&inventory);
		let found: Vec<_> = patterns(&inventory, &document)
			.into_iter()
			.map(matches)
			.collect();
		assert_eq!(found, [vec![own], vec!["docs/x/a.rs".to_owned()]]);
	}
}

#[test]
fn a_documents_directory_is_escaped_in_the_resolved_glob() {
	let inventory = inventory(&[
		("docs/[draft]/guide.md", &with_patterns(&["*.rs"])),
		("docs/[draft]/code.rs", ""),
	]);
	assert_eq!(
		patterns(&inventory, "docs/[draft]/guide.md")[0].glob,
		"/docs/[[]draft[]]/*.rs"
	);
}

#[test]
fn an_authored_class_still_matches_a_literal_wildcard_under_such_a_directory() {
	let inventory = inventory(&[
		("docs/a*b/guide.md", &with_patterns(&["lit[*].rs"])),
		("docs/a*b/lit*.rs", ""),
		("docs/a*b/litX.rs", ""),
	]);
	assert_clean(&inventory);
	assert_eq!(
		matches(patterns(&inventory, "docs/a*b/guide.md")[0]),
		["docs/a*b/lit*.rs"]
	);
}

#[test]
fn a_projects_directory_is_literal_in_its_rooted_patterns() {
	let project = path("p[1]");
	let glob =
		SourceGlob::compile_project(&project, Some(&path("p[1]/a.md")), "/*.rs")
			.unwrap();
	assert!(glob.is_match(&path("p[1]/code.rs")));
	assert!(!glob.is_match(&path("p1/code.rs")));
	assert_eq!(glob.rooted(), "/p[[]1[]]/*.rs");
}

#[test]
fn a_pattern_keeps_its_attested_members_beside_its_current_matches() {
	let declaration = format!(
		r#"{{"sources": [{{"members": [{{"oid": {OID}, "path": "/db/0001.sql", "reviewed": {REVIEW}}}, {{"oid": {OID}, "path": "../db/0002.sql", "reviewed": {REVIEW}}}], "pattern": "/db/*.sql"}}]}}"#
	);
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("policy.md", &declaration)], &[]),
		),
		("docs/policy.md", ""),
		("db/0002.sql", ""),
		("db/0003.sql", ""),
	]);
	assert_clean(&inventory);
	let [pattern] = patterns(&inventory, "docs/policy.md")[..] else {
		panic!("expected one pattern");
	};
	assert_eq!(matches(pattern), ["db/0002.sql", "db/0003.sql"]);
	let members: Vec<_> = pattern.members.keys().cloned().collect();
	assert_eq!(members, paths(&["db/0001.sql", "db/0002.sql"]));
}

#[test]
fn a_source_that_resolves_to_its_own_document_is_a_configuration_error() {
	let inventory = inventory(&[
		("docs/a.md", &markdown("  sources:\n    - path: './a.md'\n")),
		("notes/b.md", &with_patterns(&["*.md"])),
		("notes/c.md", ""),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::SelfDependency);
			assert_eq!(diagnostic.severity(), Severity::Configuration);
			(
				diagnostic.location.file.to_string(),
				diagnostic.location.field.clone().unwrap(),
			)
		})
		.collect();
	assert_eq!(
		found,
		[
			(
				"docs/a.md".to_owned(),
				"source-sentry.sources[0].path".to_owned()
			),
			(
				"notes/b.md".to_owned(),
				"source-sentry.sources[0].pattern".to_owned()
			),
		]
	);
	// No dependency edge is invented for a self-dependency.
	assert_eq!(inventory.dependencies, []);
}

#[test]
fn a_pattern_without_matches_is_a_warning() {
	let inventory =
		inventory(&[("docs/a.md", &with_patterns(&["/tests/**/*.json"]))]);
	let diagnostic = only(&inventory, DiagnosticKind::EmptyPattern);
	assert_eq!(diagnostic.severity(), Severity::Warning);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("source-sentry.sources[0].pattern")
	);
	assert!(!inventory.has_errors());
	assert_eq!(
		patterns(&inventory, "docs/a.md")[0].matches,
		Some(BTreeSet::new())
	);
}

#[test]
fn patterns_that_cannot_be_resolved_or_compiled_are_configuration_errors() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["../../*.md", "/src/[", "/src/*/../a.rs", "C:/x/*.md"]),
		),
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[("b.md", r#"{"sources": [{"pattern": "src\\*.rs"}]}"#)],
				&[],
			),
		),
		("docs/b.md", ""),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::InvalidPattern);
			diagnostic.location.field.clone().unwrap()
		})
		.collect();
	assert_eq!(
		found,
		[
			"source-sentry.sources[0].pattern",
			"source-sentry.sources[1].pattern",
			"source-sentry.sources[2].pattern",
			"source-sentry.sources[3].pattern",
			r#"documents["b.md"].sources[0].pattern"#,
		]
	);
	let messages: Vec<&str> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| diagnostic.message.as_str())
		.collect();
	assert!(messages[0].contains("climbs above the repository root"));
	assert!(messages[1].contains("unclosed character class"));
	assert!(messages[2].contains("follows a wildcard"));
	assert!(messages[3].contains("filesystem path"));
	assert!(messages[4].contains("backslash"));
	assert!(
		inventory
			.claims
			.iter()
			.all(|claim| claim.sources.is_empty())
	);
}

#[test]
fn a_missing_named_source_is_an_error_except_for_a_sealed_record() {
	let sealed_pattern = format!(
		r#"{{"seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {OID}}}, "sources": [{{"pattern": "/gone/*.rs"}}]}}"#
	);
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("live.md", &with_source("/src/gone.rs")),
					("record.md", &sealed_with_source("/src/gone.rs")),
					("record-pattern.md", &sealed_pattern),
				],
				&[],
			),
		),
		("docs/live.md", ""),
		("docs/record.md", ""),
		("docs/record-pattern.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::MissingSource);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(diagnostic.document, Some(local("docs/live.md")));
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some(r#"documents["live.md"].sources[0].path"#)
	);
	// The record's missing source is recorded but not reported, and its
	// pattern is not expanded.
	let record = inventory
		.claims_of(&local("docs/record.md"))
		.next()
		.unwrap();
	let [ResolvedSource::Path(source)] = record.sources.as_slice() else {
		panic!("expected one path source");
	};
	assert!(!source.exists);
	assert_eq!(
		patterns(&inventory, "docs/record-pattern.md")[0].matches,
		None
	);
}

#[test]
fn a_remote_documents_sources_resolve_from_the_root() {
	let declaration =
		r#"{"sources": [{"path": "/src/a.rs"}, {"pattern": "/src/*.rs"}]}"#;
	let inventory = inventory(&[
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", declaration)], &[]),
		),
		("src/a.rs", ""),
		("src/b.rs", ""),
	]);
	assert_clean(&inventory);
	let claim = &inventory.claims[0];
	let resolved: Vec<_> = claim.source_paths().into_iter().cloned().collect();
	assert_eq!(resolved, paths(&["src/a.rs", "src/b.rs"]));
}

#[test]
fn a_lockfile_tracked_document_used_as_a_source_is_an_edge() {
	let inventory = inventory(&[
		(
			"findings/source-sentry.lock",
			&lockfile(&[("results.md", &with_source("/tests/out.json"))], &[]),
		),
		("findings/results.md", ""),
		("tests/out.json", ""),
		(
			"decisions/choice.md",
			&markdown("  sources:\n    - pattern: '/findings/*.md'\n"),
		),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(
				&[("abstract:video", &with_source("/findings/results.md"))],
				&[],
			),
		),
	]);
	assert_clean(&inventory);
	let edges: Vec<_> = inventory
		.dependencies
		.iter()
		.map(|edge| (edge.dependent.to_string(), edge.upstream.to_string()))
		.collect();
	assert_eq!(
		edges,
		[
			(
				"decisions/choice.md".to_owned(),
				"findings/results.md".to_owned()
			),
			(
				"abstract:video".to_owned(),
				"findings/results.md".to_owned()
			),
		]
	);
	let upstreams: Vec<_> = inventory
		.upstreams_of(&local("decisions/choice.md"))
		.collect();
	assert_eq!(upstreams, [&path("findings/results.md")]);
}

#[test]
fn an_on_file_document_used_as_a_source_is_a_configuration_error() {
	let inventory = inventory(&[
		("findings/results.md", &markdown("  owners: ['qa']\n")),
		(
			"decisions/source-sentry.lock",
			&lockfile(&[("choice.md", &with_source("/findings/results.md"))], &[]),
		),
		("decisions/choice.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::OnFileSource);
	assert_eq!(diagnostic.document, Some(local("findings/results.md")));
	assert_eq!(diagnostic.location.file, path("findings/results.md"));
	assert_eq!(
		diagnostic.related[0].field.as_deref(),
		Some(r#"documents["choice.md"].sources[0]"#)
	);
	assert!(diagnostic.message.contains("track --from"));
	assert_eq!(inventory.dependencies, []);
}

#[test]
fn documents_that_are_sources_of_each_other_form_a_cycle() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("a.md", &with_source("b.md")),
					("b.md", &with_source("c.md")),
					("c.md", &with_source("a.md")),
					("d.md", &with_source("a.md")),
				],
				&[],
			),
		),
		("docs/a.md", ""),
		("docs/b.md", ""),
		("docs/c.md", ""),
		("docs/d.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::DependencyCycle);
	assert_eq!(diagnostic.document, Some(local("docs/a.md")));
	assert!(
		diagnostic
			.message
			.contains("`docs/a.md` → `docs/b.md` → `docs/c.md` → `docs/a.md`"),
		"{}",
		diagnostic.message
	);
	assert!(
		!diagnostic.message.contains("Other cycles"),
		"{}",
		diagnostic.message
	);
	assert_eq!(diagnostic.related.len(), 2);
	assert_eq!(inventory.dependencies.len(), 4);
	assert!(inventory.dependencies.contains(&Dependency {
		dependent: local("docs/d.md"),
		upstream: path("docs/a.md"),
	}));
}

/// The cycle message of a lockfile in `docs` whose documents have the given
/// sources, all of them also in that lockfile.
fn cycle_message(documents: &[(&str, &[&str])]) -> String {
	let entries: Vec<(String, String)> = documents
		.iter()
		.map(|(name, sources)| {
			let sources: Vec<String> = sources
				.iter()
				.map(|source| format!(r#"{{"path": "{source}"}}"#))
				.collect();
			(
				(*name).to_owned(),
				format!(r#"{{"sources": [{}]}}"#, sources.join(", ")),
			)
		})
		.collect();
	let entries: Vec<(&str, &str)> = entries
		.iter()
		.map(|(name, declaration)| (name.as_str(), declaration.as_str()))
		.collect();
	let mut files = vec![(
		"docs/source-sentry.lock".to_owned(),
		lockfile(&entries, &[]),
	)];
	files.extend(
		documents
			.iter()
			.map(|(name, _)| (format!("docs/{name}"), String::new())),
	);
	let files: Vec<(&str, &str)> = files
		.iter()
		.map(|(path, contents)| (path.as_str(), contents.as_str()))
		.collect();
	only(&inventory(&files), DiagnosticKind::DependencyCycle)
		.message
		.clone()
}

#[test]
fn a_group_with_more_than_one_cycle_says_that_others_may_remain() {
	let message = cycle_message(&[
		("a.md", &["b.md", "c.md"]),
		("b.md", &["a.md"]),
		("c.md", &["a.md"]),
	]);
	assert_eq!(
		message,
		"documents are sources of each other in a cycle: `docs/a.md` → \
		 `docs/b.md` → `docs/a.md`. Remove one of these sources to break it. \
		 Other cycles may remain among these documents, and will be reported \
		 once this one is broken"
	);
}

#[test]
fn a_group_that_is_a_single_cycle_names_it_alone() {
	let message = cycle_message(&[("a.md", &["b.md"]), ("b.md", &["a.md"])]);
	assert_eq!(
		message,
		"documents are sources of each other in a cycle: `docs/a.md` → \
		 `docs/b.md` → `docs/a.md`. Remove one of these sources to break it"
	);
}

#[test]
fn a_document_that_is_its_own_source_is_not_a_cycle() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", &with_source("a.md"))], &[]),
		),
		("docs/a.md", ""),
	]);
	assert_eq!(kinds(&inventory), [DiagnosticKind::SelfDependency]);
	assert_eq!(
		inventory.diagnostics[0].message,
		"`a.md` resolves to `docs/a.md` itself, and a document cannot be its \
		 own source. Remove the source, or narrow the pattern"
	);
}

#[test]
fn a_sealed_record_adds_no_dependency() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("audit.md", &sealed_with_source("findings.md")),
					("findings.md", "{}"),
				],
				&[],
			),
		),
		("docs/audit.md", ""),
		("docs/findings.md", ""),
	]);
	assert_eq!(kinds(&inventory), []);
	assert_eq!(inventory.dependencies, []);
}

/// The paths a declaration of `document` resolves to, in declared order.
fn resolved(inventory: &Inventory, document: &str) -> Vec<String> {
	let claim = inventory
		.claims
		.iter()
		.find(|claim| claim.document == local(document))
		.unwrap_or_else(|| panic!("no claim for {document}"));
	claim
		.sources
		.iter()
		.map(|source| match source {
			ResolvedSource::Path(resolved) => resolved.path.to_string(),
			ResolvedSource::Pattern(pattern) => pattern.glob.clone(),
		})
		.collect()
}

/// Repeated sources are found by the files they name in the document's own
/// project: in a nested project, `/` starts at that project's root and `:/`
/// at the repository's. A declaration in frontmatter and one in a lockfile
/// accept and refuse the same pairs.
#[test]
fn repeated_sources_are_compared_in_the_documents_project() {
	let declaration = |first: &str, second: &str| {
		format!("  sources:\n    - path: '{first}'\n    - path: '{second}'\n")
	};
	let entry = |first: &str, second: &str| {
		format!(r#"{{"sources": [{{"path": "{first}"}}, {{"path": "{second}"}}]}}"#)
	};
	let files = |project: &str, first: &str, second: &str, on_file: bool| {
		let guide = if project.is_empty() {
			"guide.md".to_owned()
		} else {
			format!("{project}/guide.md")
		};
		let mut files = vec![
			("src/a.rs".to_owned(), "root\n".to_owned()),
			(format!("{project}/src/a.rs"), "nested\n".to_owned()),
		];
		if !project.is_empty() {
			files.push((
				format!("{project}/.source-sentry/source-sentry.yaml"),
				"version: 1\n".to_owned(),
			));
		}
		if on_file {
			files.push((guide, markdown(&declaration(first, second))));
		} else {
			let lockfile_path = if project.is_empty() {
				"source-sentry.lock".to_owned()
			} else {
				format!("{project}/source-sentry.lock")
			};
			files.push((guide, "# Guide\n".to_owned()));
			files.push((
				lockfile_path,
				lockfile(&[("guide.md", &entry(first, second))], &[]),
			));
		}
		files
	};
	let discover = |project: &str, first: &str, second: &str, on_file: bool| {
		let files = files(project, first, second, on_file);
		let files: Vec<(&str, &str)> = files
			.iter()
			.map(|(file, contents)| (file.trim_start_matches('/'), contents.as_str()))
			.collect();
		project_inventory(&files, project)
	};

	for on_file in [true, false] {
		// In project `product`, the project's file and the repository's.
		let distinct = discover("product", "/src/a.rs", ":/src/a.rs", on_file);
		assert_clean(&distinct);
		assert_eq!(
			resolved(&distinct, "product/guide.md"),
			["product/src/a.rs", "src/a.rs"],
			"on-file: {on_file}"
		);

		// One file of the project, spelled from its root and from beside the
		// document, which lies at that root.
		let repeated = discover("product", "/src/a.rs", "src/a.rs", on_file);
		let kind = if on_file {
			DiagnosticKind::InvalidDeclaration
		} else {
			DiagnosticKind::InvalidLockfile
		};
		let diagnostic = only(&repeated, kind);
		assert!(
			diagnostic.message.contains("both name `product/src/a.rs`"),
			"on-file: {on_file}: {}",
			diagnostic.message
		);

		// At the repository's root project, `/` and `:/` start at the same
		// root, so both pairs name one file.
		for second in [":/src/a.rs", "src/a.rs"] {
			let root = discover("", "/src/a.rs", second, on_file);
			let diagnostic = only(&root, kind);
			assert!(
				diagnostic.message.contains("both name `src/a.rs`"),
				"on-file: {on_file}: {}",
				diagnostic.message
			);
		}
	}
}

/// Two patterns of one declaration are one pattern when they are spelled
/// alike once each is resolved from where it is declared, as two spellings
/// of one path name one file. The pair is refused with the diagnostic of a
/// repeated path in frontmatter, in a local lockfile and in a remote owner
/// lockfile, in the repository's root project and in a nested one.
#[test]
fn repeated_patterns_are_compared_in_the_documents_project() {
	#[derive(Clone, Copy, Debug)]
	enum Site {
		OnFile,
		Local,
		Remote,
	}
	let discover = |project: &str, site: Site, first: &str, second: &str| {
		let at = |file: &str| {
			if project.is_empty() {
				file.to_owned()
			} else {
				format!("{project}/{file}")
			}
		};
		let entry = format!(
			r#"{{"sources": [{{"pattern": "{first}"}}, {{"pattern": "{second}"}}]}}"#
		);
		let mut files = vec![
			(at("docs/src/a.rs"), "fn a() {}\n".to_owned()),
			(at("src/a.rs"), "fn b() {}\n".to_owned()),
			("docs/src/a.rs".to_owned(), "fn c() {}\n".to_owned()),
		];
		if !project.is_empty() {
			files.push((
				at(".source-sentry/source-sentry.yaml"),
				"version: 1\n".to_owned(),
			));
		}
		match site {
			Site::OnFile => files.push((
				at("docs/guide.md"),
				markdown(&format!(
					"  sources:\n    - pattern: '{first}'\n    - pattern: \
					 '{second}'\n"
				)),
			)),
			Site::Local => {
				files.push((at("docs/guide.md"), "# Guide\n".to_owned()));
				files.push((
					at("docs/source-sentry.lock"),
					lockfile(&[("guide.md", &entry)], &[]),
				));
			}
			Site::Remote => files.push((
				at(".source-sentry/remotes/legal.lock"),
				lockfile(&[("abstract:video", &entry)], &[]),
			)),
		}
		let files: Vec<(&str, &str)> = files
			.iter()
			.map(|(file, contents)| (file.as_str(), contents.as_str()))
			.collect();
		project_inventory(&files, project)
	};

	for project in ["", "product"] {
		let nested = |file: &str| {
			if project.is_empty() {
				file.to_owned()
			} else {
				format!("{project}/{file}")
			}
		};
		let target = nested("docs/src/*.rs");
		let repository_rooted = format!(":/{target}");
		for site in [Site::OnFile, Site::Local, Site::Remote] {
			let (kind, field) = match site {
				Site::OnFile => (
					DiagnosticKind::InvalidDeclaration,
					"source-sentry".to_owned(),
				),
				Site::Local => (
					DiagnosticKind::InvalidLockfile,
					"documents[\"guide.md\"]".to_owned(),
				),
				Site::Remote => (
					DiagnosticKind::InvalidLockfile,
					"documents[\"abstract:video\"]".to_owned(),
				),
			};
			let rooted = [
				("/docs/src/*.rs", "/docs/src/*.rs"),
				("/docs/src/*.rs", repository_rooted.as_str()),
				("/docs/./src/*.rs", "/docs//src/*.rs"),
				("/docs/src/*.rs", "/src/../docs/src/*.rs"),
			];
			let relative = [
				("src/*.rs", "/docs/src/*.rs"),
				("src/*.rs", repository_rooted.as_str()),
				("./src/*.rs", "../docs/src/*.rs"),
			];
			let repeated: Vec<_> = match site {
				Site::Remote => rooted.to_vec(),
				Site::OnFile | Site::Local => {
					rooted.into_iter().chain(relative).collect()
				}
			};
			for (first, second) in repeated {
				let inventory = discover(project, site, first, second);
				let diagnostic = only(&inventory, kind);
				assert_eq!(
					diagnostic.location.field.as_deref(),
					Some(format!("{field}.sources[1].pattern").as_str()),
					"{site:?} in `{project}`: `{first}` and `{second}`"
				);
				assert!(
					diagnostic.message.contains(&format!(
						"`{field}.sources[1].pattern` repeats \
						 `{field}.sources[0].pattern`: both name `{target}`"
					)),
					"{site:?} in `{project}`: `{first}` and `{second}`: {}",
					diagnostic.message
				);
			}

			// Patterns that are spelled differently once resolved stay
			// distinct, even when they match the same files.
			let rooted = [
				("/docs/src/*.rs", "/docs/src/[*].rs"),
				("/docs/src/*.rs", "/docs/src/a.rs"),
				("/docs/src/{a,b}.rs", "/docs/src/{b,a}.rs"),
				("/docs/src/*.rs", "/src/*.rs"),
				("/docs/src/*.rs", ":/docs/src/*.rs"),
			];
			let relative = [
				("src/*.rs", "/src/*.rs"),
				("src/*.rs", "src/[*].rs"),
				("src/*.rs", "./src/a.rs"),
			];
			let distinct: Vec<_> = match site {
				Site::Remote => rooted.to_vec(),
				Site::OnFile | Site::Local => {
					rooted.into_iter().chain(relative).collect()
				}
			};
			for (first, second) in distinct {
				if project.is_empty() && second.starts_with(":/") {
					// At the root project, `/` and `:/` start at the same root.
					continue;
				}
				let inventory = discover(project, site, first, second);
				assert!(
					!kinds(&inventory).contains(&kind),
					"{site:?} in `{project}`: `{first}` and `{second}`: {:#?}",
					inventory.diagnostics
				);
				assert_eq!(
					inventory
						.claims
						.iter()
						.map(|claim| claim.declaration.sources.len())
						.collect::<Vec<_>>(),
					[2],
					"{site:?} in `{project}`: `{first}` and `{second}`"
				);
			}
		}
	}
}

#[test]
fn a_source_path_that_does_not_resolve_is_a_configuration_error() {
	// The adapters reject the whole declaration while reading it.
	let inventory = inventory(&[(
		"docs/a.md",
		&markdown("  sources:\n    - path: '../../outside.rs'\n"),
	)]);
	let diagnostic = only(&inventory, DiagnosticKind::InvalidDeclaration);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("source-sentry.sources[0].path")
	);

	// Resolution reports the same problem for a declaration that reaches it.
	let mut claims = [Claim {
		document: local("a.md"),
		mode: Mode::OnFile,
		declared_in: path("a.md"),
		field: "source-sentry".to_owned(),
		declaration: Declaration {
			sources: vec![
				SourceDecl::Path(PathSource {
					oid: None,
					path: "../outside.rs".to_owned(),
					reviewed: None,
				}),
				SourceDecl::Pattern(PatternSource {
					members: Vec::new(),
					pattern: "/*.rs".to_owned(),
					reviewed: None,
				}),
			],
			..Declaration::default()
		},
		sources: Vec::new(),
		presence: Presence::Present,
		lines: BTreeMap::new(),
	}];
	let mut diagnostics = Vec::new();
	let tracked = BTreeSet::from([path("a.md"), path("b.rs")]);
	resolve(
		&mut claims,
		&tracked,
		&std::collections::BTreeSet::from([crate::model::RepoPath::ROOT]),
		&mut diagnostics,
	);
	let found: Vec<_> = diagnostics
		.iter()
		.map(|diagnostic| (diagnostic.kind, diagnostic.location.field.clone()))
		.collect();
	assert_eq!(
		found,
		[(
			DiagnosticKind::InvalidSourcePath,
			Some("source-sentry.sources[0].path".to_owned())
		)]
	);
	// The unresolvable source is left out; the rest keep their index.
	let indices: Vec<_> = claims[0]
		.sources
		.iter()
		.map(ResolvedSource::index)
		.collect();
	assert_eq!(indices, [1]);
}
